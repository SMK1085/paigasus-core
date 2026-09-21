// SPDX-License-Identifier: Apache-2.0
//
// The IAM that the e2e tier talks to: one organization with a team and a project, a grant on a
// project in a second organization, and an audit entry. `worldHandlers()` always returns the FULL
// handler set, because the fake's setHandlers() REPLACES the whole map (Task 11). Every key an
// override uses must therefore exist in the default set below.
//
// One piece of state: an organization that CreateOrganization makes is in every later
// ListOrganizations answer of the same world. Each worldHandlers() call starts with none, so a
// test cannot see the organizations of an earlier test. R6 uses it to prove that the action
// refreshes the page (P5b-16).
//
// SMA-629: three outbox handlers are in the default set too, because every override key must exist
// there. They hold two seeded dead letters with RFC 4122 ids, per worldHandlers() call; replay and
// discard remove the entry, and an unknown id answers NotFound, as IAM does.
//
// PRNs are literal strings: @paigasus/console-core's prn-tenancy.ts imports server-only, which
// throws under Playwright. NodeStatus comes from @paigasus/sdk's guard-free ./iam/types entry, and
// IamAction is a TYPE import, which the compiler erases (SMA-630).
import { Code } from '@connectrpc/connect';
import type { IamAction } from '@paigasus/console-core';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { NodeStatus } from '@paigasus/sdk/iam/types';

export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e2';
export const PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e3';
export const OTHER_ORG_ID = '0190a100-0000-7000-8000-0000000000e4';
export const OTHER_TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e5';
export const OTHER_PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e6';
export const NEW_ORG_ID = '0190a100-0000-7000-8000-0000000000e7';

export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
export const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;
export const OTHER_TEAM_PRN = `prn:pgs:iam::${OTHER_ORG_ID}:team/${OTHER_TEAM_ID}`;
export const OTHER_PROJECT_PRN = `prn:pgs:iam::${OTHER_ORG_ID}:project/${OTHER_PROJECT_ID}`;

export const ORG_NAME = 'Acme Research';
export const TEAM_NAME = 'Platform Team';
export const PROJECT_NAME = 'Inference Gateway';
export const OTHER_PROJECT_NAME = 'Shared Models';
export const AUDIT_ACTION = 'CreateTeam';

/**
 * The Cedar action names the app asks IsAuthorized about. `satisfies` holds each entry to
 * @paigasus/console-core's IamAction, and tests/unit/world-actions.test.ts holds the SET to
 * IAM_ACTIONS (SMA-630 spec § 5.1).
 */
export const ALL_ACTIONS = [
  'ListOrganizations',
  'CreateOrganization',
  'RenameOrganization',
  'ArchiveOrganization',
  'RestoreOrganization',
  'CreateTeam',
  'RenameTeam',
  'ArchiveTeam',
  'RestoreTeam',
  'CreateProject',
  'RenameProject',
  'ArchiveProject',
  'RestoreProject',
  'AttachMembership',
  'DetachMembership',
  'ListAuditLog',
  // SMA-629: the Dead letters nav entry.
  'ListOutboxDeadLetters',
  // SMA-661: the replay affordance of the dead-letters page. Without it the default world DENIES
  // replay, the page hides every Replay button, and R17 fails at its Replay click.
  'ReplayOutboxDeadLetter',
  // SMA-636: the gateway settings. This zone asks none of them, but the SET must equal IAM_ACTIONS.
  'CreateServiceAccount',
  'ArchiveServiceAccount',
  'IssueApiKey',
  'RevokeApiKey',
  'GrantRole',
] as const satisfies readonly IamAction[];

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.audit'] };

export const DEAD_LETTER_A_ID = '0190a1f0-0000-7000-8000-0000000000d1';
export const DEAD_LETTER_B_ID = '0190a1f0-0000-7000-8000-0000000000d2';

/**
 * An IAM that serves OutboxService (SMA-629). The FULL Descriptor shape — service, version and
 * capabilities — because the type needs all three. DEFAULT_DESCRIPTOR keeps its two keys, so a
 * test that does not ask for this one meets an older IAM (AC 2).
 */
export const DEAD_LETTERS_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.audit', 'iam.deadletters'] };

export type WorldOptions = {
  /** The actions IsAuthorized allows. Default: all of them. */
  readonly allow?: readonly string[];
  /** false: a first-time identity with no membership and no grant. Default: true. */
  readonly memberships?: boolean;
  /** What GET /v1/service-info answers. Default: DEFAULT_DESCRIPTOR. */
  readonly descriptor?: Descriptor;
  /** Replace single handlers, for example with one that throws denial(). */
  readonly overrides?: FakeIamHandlers;
};

const notFound = (): Error => denial({ code: Code.NotFound, reason: 'not-found' });

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

function seededDeadLetters(): Map<string, DeadLetterFixture> {
  const entries: DeadLetterFixture[] = [
    {
      id: DEAD_LETTER_A_ID,
      occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
      eventType: 'iam.team.created',
      schemaVersion: 1,
      aggregatePrn: TEAM_PRN,
      actorPrn: PRINCIPAL_PRN,
      payload: '{"slug":"platform"}',
      correlationId: 'corr-dead-letter-a',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_300n, nanos: 0 },
      lastError: 'nats: no responders available for request',
    },
    {
      id: DEAD_LETTER_B_ID,
      occurredAt: { seconds: 1_788_000_600n, nanos: 0 },
      eventType: 'iam.project.created',
      schemaVersion: 1,
      aggregatePrn: PROJECT_PRN,
      actorPrn: '',
      payload: '{"slug":"gateway"}',
      correlationId: '',
      attempts: 5,
      parkedAt: { seconds: 1_788_000_900n, nanos: 0 },
      lastError: '',
    },
  ];
  return new Map(entries.map((entry) => [entry.id, entry]));
}

function takeDeadLetter(deadLetters: Map<string, DeadLetterFixture>, id: string): DeadLetterFixture {
  const entry = deadLetters.get(id);
  if (entry === undefined) throw notFound();
  deadLetters.delete(id);
  return entry;
}

/**
 * Every scripted node is active. Without a status a node reads as UNSPECIFIED, and every row and
 * header would show "Status unknown" (SMA-630 spec § 8).
 */
const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };

// Named constants, not `Map.get()` results, where a response lists them: the handlers are typed per
// method (Task 11), and a repeated field must not hold `undefined`.
const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME, ...ACTIVE };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME, ...ACTIVE };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME, ...ACTIVE };

const ORGANIZATIONS = new Map([[ORG_PRN, ORGANIZATION]]);
const TEAMS = new Map([
  [TEAM_PRN, TEAM],
  [OTHER_TEAM_PRN, { prn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'shared', name: 'Shared Team', ...ACTIVE }],
]);
const PROJECTS = new Map([
  [PROJECT_PRN, PROJECT],
  [OTHER_PROJECT_PRN, { prn: OTHER_PROJECT_PRN, teamPrn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'models', name: OTHER_PROJECT_NAME, ...ACTIVE }],
]);

function organizationAt(prn: string): typeof ORGANIZATION {
  const organization = ORGANIZATIONS.get(prn);
  if (organization === undefined) throw notFound();
  return organization;
}

function teamAt(prn: string): typeof TEAM {
  const team = TEAMS.get(prn);
  if (team === undefined) throw notFound();
  return team;
}

function projectAt(prn: string): typeof PROJECT {
  const project = PROJECTS.get(prn);
  if (project === undefined) throw notFound();
  return project;
}

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const allow = new Set<string>(options.allow ?? ALL_ACTIONS);
  const withScopes = options.memberships ?? true;
  const created: { prn: string; slug: string; name: string; status: NodeStatus; effectiveStatus: NodeStatus }[] = [];
  const deadLetters = seededDeadLetters();
  const e2ePrincipal = () => ({
    principalPrn: PRINCIPAL_PRN,
    status: 'active',
    issuer: 'fake-idp',
    subject: 'e2e-user',
    memberships: withScopes
      ? [
          { id: '0190a1d4-0000-7000-8000-0000000000f1', principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN },
          { id: '0190a1d4-0000-7000-8000-0000000000f2', principalPrn: PRINCIPAL_PRN, nodePrn: TEAM_PRN },
        ]
      : [],
  });
  return {
    // Both authn reads answer the SAME principal. The console calls whoAmI (SMA-632); introspect
    // stays scripted because IAM still serves it and a test may drive it directly.
    'authn.introspect': e2ePrincipal,
    'authn.whoAmI': e2ePrincipal,
    'authz.isAuthorized': (req: { action: string }) => ({ allowed: allow.has(req.action), determiningPolicies: [], reason: '' }),
    'authz.listRoleGrants': () => ({
      grants: withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: OTHER_PROJECT_PRN }] : [],
    }),
    'tenancy.getOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.getTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.getProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.listOrganizations': () => ({ organizations: [...ORGANIZATIONS.values(), ...created] }),
    'tenancy.listTeams': () => ({ teams: [TEAM] }),
    'tenancy.listProjects': () => ({ projects: [PROJECT] }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'tenancy.createOrganization': (req: { slug: string; name: string }) => {
      const organization = { prn: `prn:pgs:iam:::organization/${NEW_ORG_ID}`, slug: req.slug, name: req.name, ...ACTIVE };
      created.push(organization);
      return { organization };
    },
    'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_PRN, orgPrn: req.orgPrn, slug: req.slug, name: req.name, ...ACTIVE } }),
    'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({
      project: { prn: PROJECT_PRN, teamPrn: req.teamPrn, orgPrn: ORG_PRN, slug: req.slug, name: req.name, ...ACTIVE },
    }),
    // SMA-630: the nine lifecycle RPCs. Each returns the scripted node unchanged. A test that needs
    // the node to change (R14) scripts stateful handlers through `overrides`.
    'tenancy.renameOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.archiveOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.restoreOrganization': (req: { prn: string }) => ({ organization: organizationAt(req.prn) }),
    'tenancy.renameTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.archiveTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.restoreTeam': (req: { prn: string }) => ({ team: teamAt(req.prn) }),
    'tenancy.renameProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.archiveProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.restoreProject': (req: { prn: string }) => ({ project: projectAt(req.prn) }),
    'tenancy.attachMembership': (req: { principalPrn: string; nodePrn: string }) => ({
      membership: { id: '0190a1d4-0000-7000-8000-0000000000f5', principalPrn: req.principalPrn, nodePrn: req.nodePrn },
    }),
    'tenancy.detachMembership': () => ({}),
    'audit.listAuditEntries': () => ({
      entries: [
        {
          id: 'audit-e2e-1',
          occurredAt: { seconds: 1_788_000_000n, nanos: 0 },
          actorPrn: PRINCIPAL_PRN,
          action: AUDIT_ACTION,
          resourcePrn: ORG_PRN,
          outcome: 'allow',
          determiningPolicies: [],
          detailJson: '{}',
          correlationId: 'corr-audit-e2e',
        },
      ],
      nextCursor: '',
    }),
    // SMA-629. IAM orders by id DESCENDING and matches event_type exactly.
    'outbox.listDeadLetters': (req: { eventType: string }) => ({
      entries: [...deadLetters.values()].filter((entry) => req.eventType === '' || entry.eventType === req.eventType).sort((a, b) => (a.id < b.id ? 1 : -1)),
      nextCursor: '',
    }),
    'outbox.replayDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(deadLetters, req.id) }),
    'outbox.discardDeadLetter': (req: { id: string }) => ({ entry: takeDeadLetter(deadLetters, req.id) }),
    ...options.overrides,
  };
}
