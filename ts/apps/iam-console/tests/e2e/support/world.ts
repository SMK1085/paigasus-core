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
// PRNs are literal strings: lib/prn.ts imports server-only, which throws under Playwright.
import { Code } from '@connectrpc/connect';
import { denial, type FakeIamHandlers } from '../../support/fake-iam';

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

/** The Cedar action names the app asks IsAuthorized about (lib/authorize.ts IamAction). */
export const ALL_ACTIONS = ['ListOrganizations', 'CreateOrganization', 'CreateTeam', 'CreateProject', 'AttachMembership', 'DetachMembership', 'ListAuditLog'] as const;

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.audit'] };

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

// Named constants, not `Map.get()` results, where a response lists them: the handlers are typed per
// method (Task 11), and a repeated field must not hold `undefined`.
const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME };

const ORGANIZATIONS = new Map([[ORG_PRN, ORGANIZATION]]);
const TEAMS = new Map([
  [TEAM_PRN, TEAM],
  [OTHER_TEAM_PRN, { prn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'shared', name: 'Shared Team' }],
]);
const PROJECTS = new Map([
  [PROJECT_PRN, PROJECT],
  [OTHER_PROJECT_PRN, { prn: OTHER_PROJECT_PRN, teamPrn: OTHER_TEAM_PRN, orgPrn: `prn:pgs:iam:::organization/${OTHER_ORG_ID}`, slug: 'models', name: OTHER_PROJECT_NAME }],
]);

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const allow = new Set<string>(options.allow ?? ALL_ACTIONS);
  const withScopes = options.memberships ?? true;
  const created: { prn: string; slug: string; name: string }[] = [];
  return {
    'authn.introspect': () => ({
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
    }),
    'authz.isAuthorized': (req: { action: string }) => ({ allowed: allow.has(req.action), determiningPolicies: [], reason: '' }),
    'authz.listRoleGrants': () => ({
      grants: withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: OTHER_PROJECT_PRN }] : [],
    }),
    'tenancy.getOrganization': (req: { prn: string }) => {
      const organization = ORGANIZATIONS.get(req.prn);
      if (organization === undefined) throw notFound();
      return { organization };
    },
    'tenancy.getTeam': (req: { prn: string }) => {
      const team = TEAMS.get(req.prn);
      if (team === undefined) throw notFound();
      return { team };
    },
    'tenancy.getProject': (req: { prn: string }) => {
      const project = PROJECTS.get(req.prn);
      if (project === undefined) throw notFound();
      return { project };
    },
    'tenancy.listOrganizations': () => ({ organizations: [...ORGANIZATIONS.values(), ...created] }),
    'tenancy.listTeams': () => ({ teams: [TEAM] }),
    'tenancy.listProjects': () => ({ projects: [PROJECT] }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'tenancy.createOrganization': (req: { slug: string; name: string }) => {
      const organization = { prn: `prn:pgs:iam:::organization/${NEW_ORG_ID}`, slug: req.slug, name: req.name };
      created.push(organization);
      return { organization };
    },
    'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_PRN, orgPrn: req.orgPrn, slug: req.slug, name: req.name } }),
    'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({ project: { prn: PROJECT_PRN, teamPrn: req.teamPrn, orgPrn: ORG_PRN, slug: req.slug, name: req.name } }),
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
    ...options.overrides,
  };
}
