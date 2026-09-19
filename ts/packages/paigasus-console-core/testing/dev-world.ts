// SPDX-License-Identifier: Apache-2.0
//
// The IAM the DEV STACK talks to (SMA-641): one organization, one team, one project, one audit
// entry, three dead letters (SMA-629), every action allowed. It models a CURRENT IAM, which always
// reports `iam.deadletters`. `startFakeIam()` with no handlers cannot serve the console — its
// defaults answer three methods and throw `Unimplemented` for every tenancy and audit call
// (fake-iam.ts:245-258) — and `setHandlers()` replaces the whole map (:126-129), so the map below
// is complete rather than partial.
//
// This is deliberately NOT the e2e world. Each app's tests/e2e/support/world.ts serves assertions
// and carries the options those assertions need; this one serves a person clicking through the
// app. Sharing one would mean an e2e assertion change altered the dev experience, and the reverse.
//
// Organizations, teams and projects live in per-devWorld()-call state, keyed by PRN, seeded with
// the three fixtures below. A developer who creates, renames, archives or restores a node sees
// that change reflected in every later get/list — the whole point of a fixture a person clicks
// through rather than a database.
import { randomUUID } from 'node:crypto';
import { Code } from '@connectrpc/connect';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import type { GatewayDescriptorBody } from './fake-gateway';
import { denial, type FakeIamHandlers, type ServiceDescriptorBody } from './fake-iam';

const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-00000000d001';
const ORG_ID = '0190a100-0000-7000-8000-00000000d002';
const TEAM_ID = '0190a1b2-0000-7000-8000-00000000d003';
const PROJECT_ID = '0190a1c3-0000-7000-8000-00000000d004';

const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;

/** Without a status a node reads as UNSPECIFIED, and every row and header shows "Status unknown". */
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

const ORGANIZATION = { prn: ORG_PRN, slug: 'dev', name: 'Dev Organization', ...ACTIVE };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: 'Platform Team', ...ACTIVE };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: 'Inference Gateway', ...ACTIVE };

type OrganizationNode = typeof ORGANIZATION;
type TeamNode = typeof TEAM;
type ProjectNode = typeof PROJECT;

/**
 * iam.authz.cedar gates myScopes() (src/scopes.ts:116-118); iam.audit gates the audit page;
 * iam.deadletters gates the dead-letters page. A current IAM always reports the last one (SMA-629 D2).
 */
export const DEV_IAM_DESCRIPTOR: ServiceDescriptorBody = { service: 'iam', version: '0.0.0-dev', capabilities: ['iam.authz.cedar', 'iam.audit', 'iam.deadletters'] };

export const DEV_GATEWAY_DESCRIPTOR: GatewayDescriptorBody = { service: 'gateway', version: '0.0.0-dev', capabilities: ['gateway.chat.stream'] };

/** A parked outbox event, as IAM's DeadLetterEntry carries it. `''` means "none" (iam.proto:619-624). */
type DeadLetterFixture = {
  id: string;
  occurredAt: { seconds: bigint; nanos: number };
  eventType: string;
  schemaVersion: number;
  aggregatePrn: string;
  actorPrn: string;
  payload: string;
  correlationId: string;
  attempts: number;
  parkedAt: { seconds: bigint; nanos: number };
  lastError: string;
};

/** Three parked events with RFC 4122 (UUIDv7-shaped) ids, like the ids IAM mints. */
function seededDeadLetters(): Map<string, DeadLetterFixture> {
  const entries: DeadLetterFixture[] = [
    {
      id: '0190a1f0-0000-7000-8000-00000000d201',
      occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
      eventType: 'iam.organization.created',
      schemaVersion: 1,
      aggregatePrn: ORG_PRN,
      actorPrn: PRINCIPAL_PRN,
      payload: '{"slug":"dev","name":"Dev Organization"}',
      correlationId: 'corr-dev-dead-letter-1',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_300n, nanos: 0 },
      lastError: 'nats: no responders available for request',
    },
    {
      id: '0190a1f0-0000-7000-8000-00000000d202',
      occurredAt: { seconds: 1_788_000_600n, nanos: 0 },
      eventType: 'iam.team.created',
      schemaVersion: 1,
      aggregatePrn: TEAM_PRN,
      actorPrn: PRINCIPAL_PRN,
      payload: '{"slug":"platform","name":"Platform Team"}',
      correlationId: 'corr-dev-dead-letter-2',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_900n, nanos: 0 },
      lastError: 'nats: timeout',
    },
    {
      id: '0190a1f0-0000-7000-8000-00000000d203',
      occurredAt: { seconds: 1_788_001_200n, nanos: 0 },
      eventType: 'iam.project.created',
      schemaVersion: 2,
      aggregatePrn: PROJECT_PRN,
      actorPrn: '',
      payload: '{"slug":"gateway","name":"Inference Gateway"}',
      correlationId: '',
      attempts: 5,
      parkedAt: { seconds: 1_788_001_500n, nanos: 0 },
      lastError: '',
    },
  ];
  return new Map(entries.map((entry) => [entry.id, entry]));
}

/** Same shape as the e2e world's `notFound()` (tests/e2e/support/world.ts): a PermissionDenied-style IAM error. */
const notFound = (): Error => denial({ code: Code.NotFound, reason: 'not-found' });

/** The resource id a tenancy PRN carries after its last `/` — used to keep a generated team or project PRN inside its org. */
function idOf(prn: string): string {
  return prn.slice(prn.lastIndexOf('/') + 1);
}

function renamed<T extends { slug: string; name: string }>(node: T, patch: { newSlug?: string | undefined; newName?: string | undefined }): T {
  return { ...node, ...(patch.newSlug !== undefined && { slug: patch.newSlug }), ...(patch.newName !== undefined && { name: patch.newName }) };
}

function archived<T extends { status: NodeStatus; effectiveStatus: NodeStatus }>(node: T): T {
  return { ...node, status: NodeStatus.ARCHIVED, effectiveStatus: NodeStatus.ARCHIVED };
}

function restored<T extends { status: NodeStatus; effectiveStatus: NodeStatus }>(node: T): T {
  return { ...node, ...ACTIVE };
}

export function devWorld(): FakeIamHandlers {
  // State is per devWorld() call: a developer's creates and renames live for the life of one dev
  // stack run and never leak between test workers.
  const organizations = new Map<string, OrganizationNode>([[ORG_PRN, ORGANIZATION]]);
  const teams = new Map<string, TeamNode>([[TEAM_PRN, TEAM]]);
  const projects = new Map<string, ProjectNode>([[PROJECT_PRN, PROJECT]]);
  // SMA-629: replay and discard remove the entry, as IAM does; an unknown id answers NotFound.
  const deadLetters = seededDeadLetters();

  function takeDeadLetter(id: string): DeadLetterFixture {
    const entry = deadLetters.get(id);
    if (entry === undefined) throw notFound();
    deadLetters.delete(id);
    return entry;
  }

  function organizationAt(prn: string): OrganizationNode {
    const organization = organizations.get(prn);
    if (organization === undefined) throw notFound();
    return organization;
  }
  function teamAt(prn: string): TeamNode {
    const team = teams.get(prn);
    if (team === undefined) throw notFound();
    return team;
  }
  function projectAt(prn: string): ProjectNode {
    const project = projects.get(prn);
    if (project === undefined) throw notFound();
    return project;
  }

  return {
    'authn.introspect': () => ({
      principalPrn: PRINCIPAL_PRN,
      status: 'active',
      issuer: 'fake-idp',
      subject: 'dev-user',
      memberships: [
        { id: '0190a1d4-0000-7000-8000-00000000d101', principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN },
        { id: '0190a1d4-0000-7000-8000-00000000d102', principalPrn: PRINCIPAL_PRN, nodePrn: TEAM_PRN },
      ],
    }),
    'authz.isAuthorized': () => ({ allowed: true, determiningPolicies: [], reason: '' }),
    'authz.listRoleGrants': () => ({ grants: [{ id: '0190a1d4-0000-7000-8000-00000000d103', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: PROJECT_PRN }] }),
    // `serviceInfo.getServiceInfo` is deliberately NOT scripted here. `dispatch()` in fake-iam.ts
    // always prefers a scripted handler over `defaults()`, so scripting it would freeze the gRPC
    // answer at whatever this map returned, even after a later `setServiceInfo()` call moved the
    // HTTP answer — breaking the fake's documented contract that a descriptor change updates both
    // the gRPC and the HTTP answer (fake-iam.ts:130-134). Leaving it unscripted lets `defaults()`
    // keep serving the mutable `grpcDescriptor`, matching the e2e world's own choice
    // (tests/e2e/support/world.ts), where the harness calls `setServiceInfo()` after startup.
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.getTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.getProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.listOrganizations': () => ({ organizations: [...organizations.values()] }),
    'tenancy.listTeams': () => ({ teams: [...teams.values()] }),
    'tenancy.listProjects': () => ({ projects: [...projects.values()] }),
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-00000000d104', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'tenancy.createOrganization': (req: { slug: string; name: string }) => {
      const organization: OrganizationNode = { prn: `prn:pgs:iam:::organization/${randomUUID()}`, slug: req.slug, name: req.name, ...ACTIVE };
      organizations.set(organization.prn, organization);
      return { organization };
    },
    'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => {
      const team: TeamNode = { prn: `prn:pgs:iam::${idOf(req.orgPrn)}:team/${randomUUID()}`, orgPrn: req.orgPrn, slug: req.slug, name: req.name, ...ACTIVE };
      teams.set(team.prn, team);
      return { team };
    },
    'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => {
      const orgPrn = teams.get(req.teamPrn)?.orgPrn ?? ORG_PRN;
      const project: ProjectNode = { prn: `prn:pgs:iam::${idOf(orgPrn)}:project/${randomUUID()}`, teamPrn: req.teamPrn, orgPrn, slug: req.slug, name: req.name, ...ACTIVE };
      projects.set(project.prn, project);
      return { project };
    },
    'tenancy.renameOrganization': (req: { prn: string; newSlug?: string | undefined; newName?: string | undefined }) => {
      const organization = renamed(organizationAt(req.prn), req);
      organizations.set(organization.prn, organization);
      return { organization };
    },
    'tenancy.archiveOrganization': (req: { prn: string }) => {
      const organization = archived(organizationAt(req.prn));
      organizations.set(organization.prn, organization);
      return { organization };
    },
    'tenancy.restoreOrganization': (req: { prn: string }) => {
      const organization = restored(organizationAt(req.prn));
      organizations.set(organization.prn, organization);
      return { organization };
    },
    'tenancy.renameTeam': (req: { prn: string; newSlug?: string | undefined; newName?: string | undefined }) => {
      const team = renamed(teamAt(req.prn), req);
      teams.set(team.prn, team);
      return { team };
    },
    'tenancy.archiveTeam': (req: { prn: string }) => {
      const team = archived(teamAt(req.prn));
      teams.set(team.prn, team);
      return { team };
    },
    'tenancy.restoreTeam': (req: { prn: string }) => {
      const team = restored(teamAt(req.prn));
      teams.set(team.prn, team);
      return { team };
    },
    'tenancy.renameProject': (req: { prn: string; newSlug?: string | undefined; newName?: string | undefined }) => {
      const project = renamed(projectAt(req.prn), req);
      projects.set(project.prn, project);
      return { project };
    },
    'tenancy.archiveProject': (req: { prn: string }) => {
      const project = archived(projectAt(req.prn));
      projects.set(project.prn, project);
      return { project };
    },
    'tenancy.restoreProject': (req: { prn: string }) => {
      const project = restored(projectAt(req.prn));
      projects.set(project.prn, project);
      return { project };
    },
    'tenancy.attachMembership': (req: { principalPrn: string; nodePrn: string }) => ({
      membership: { id: '0190a1d4-0000-7000-8000-00000000d105', principalPrn: req.principalPrn, nodePrn: req.nodePrn },
    }),
    'tenancy.detachMembership': () => ({}),
    'audit.listAuditEntries': () => ({
      entries: [
        {
          id: 'audit-dev-1',
          occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
          actorPrn: PRINCIPAL_PRN,
          action: 'CreateTeam',
          resourcePrn: ORG_PRN,
          outcome: 'allow',
          determiningPolicies: [],
          detailJson: '{}',
          correlationId: 'corr-dev-1',
        },
      ],
      nextCursor: '',
    }),
    // IAM orders by id DESCENDING (tests/dead_letters_pg.rs:432) and matches event_type exactly.
    'outbox.listDeadLetters': (req: { eventType: string }) => ({
      entries: [...deadLetters.values()].filter((entry) => req.eventType === '' || entry.eventType === req.eventType).sort((a, b) => (a.id < b.id ? 1 : -1)),
      nextCursor: '',
    }),
    'outbox.replayDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(req.id) }),
    'outbox.discardDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(req.id) }),
  };
}
