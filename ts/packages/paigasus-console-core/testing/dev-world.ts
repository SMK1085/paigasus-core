// SPDX-License-Identifier: Apache-2.0
//
// The IAM the DEV STACK talks to (SMA-641): one organization, one team, one project, one audit
// entry, every action allowed. `startFakeIam()` with no handlers cannot serve the console — its
// defaults answer three methods and throw `Unimplemented` for every tenancy and audit call
// (fake-iam.ts:245-258) — and `setHandlers()` replaces the whole map (:126-129), so the map below
// is complete rather than partial.
//
// This is deliberately NOT the e2e world. Each app's tests/e2e/support/world.ts serves assertions
// and carries the options those assertions need; this one serves a person clicking through the
// app. Sharing one would mean an e2e assertion change altered the dev experience, and the reverse.
import { randomUUID } from 'node:crypto';
import { NodeStatus } from '@paigasus/sdk/iam/types';
import type { GatewayDescriptorBody } from './fake-gateway';
import type { FakeIamHandlers, ServiceDescriptorBody } from './fake-iam';

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

/** iam.authz.cedar gates myScopes() (src/scopes.ts:116-118); iam.audit gates the audit page. */
export const DEV_IAM_DESCRIPTOR: ServiceDescriptorBody = { service: 'iam', version: '0.0.0-dev', capabilities: ['iam.authz.cedar', 'iam.audit'] };

export const DEV_GATEWAY_DESCRIPTOR: GatewayDescriptorBody = { service: 'gateway', version: '0.0.0-dev', capabilities: ['gateway.chat.stream'] };

export function devWorld(): FakeIamHandlers {
  // An organization the developer creates stays listed for the life of the stack.
  const created: (typeof ORGANIZATION)[] = [];
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
    // Scripted (rather than left to the fake's built-in default) so it stays a member of
    // Object.keys(handlers) — the completeness this world's own test pins.
    'serviceInfo.getServiceInfo': () => ({ serviceInfo: { ...DEV_IAM_DESCRIPTOR } }),
    'tenancy.getOrganization': () => ({ organization: ORGANIZATION }),
    'tenancy.getTeam': () => ({ team: TEAM }),
    'tenancy.getProject': () => ({ project: PROJECT }),
    'tenancy.listOrganizations': () => ({ organizations: [ORGANIZATION, ...created] }),
    'tenancy.listTeams': () => ({ teams: [TEAM] }),
    'tenancy.listProjects': () => ({ projects: [PROJECT] }),
    'tenancy.listMemberships': (req) => ({
      memberships: [{ id: '0190a1d4-0000-7000-8000-00000000d104', principalPrn: PRINCIPAL_PRN, nodePrn: req.filter.case === 'nodePrn' ? req.filter.value : '' }],
    }),
    'tenancy.createOrganization': (req: { slug: string; name: string }) => {
      const organization = { prn: `prn:pgs:iam:::organization/${randomUUID()}`, slug: req.slug, name: req.name, ...ACTIVE };
      created.push(organization);
      return { organization };
    },
    'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_PRN, orgPrn: req.orgPrn, slug: req.slug, name: req.name, ...ACTIVE } }),
    'tenancy.createProject': (req: { teamPrn: string; slug: string; name: string }) => ({
      project: { prn: PROJECT_PRN, teamPrn: req.teamPrn, orgPrn: ORG_PRN, slug: req.slug, name: req.name, ...ACTIVE },
    }),
    'tenancy.renameOrganization': () => ({ organization: ORGANIZATION }),
    'tenancy.archiveOrganization': () => ({ organization: ORGANIZATION }),
    'tenancy.restoreOrganization': () => ({ organization: ORGANIZATION }),
    'tenancy.renameTeam': () => ({ team: TEAM }),
    'tenancy.archiveTeam': () => ({ team: TEAM }),
    'tenancy.restoreTeam': () => ({ team: TEAM }),
    'tenancy.renameProject': () => ({ project: PROJECT }),
    'tenancy.archiveProject': () => ({ project: PROJECT }),
    'tenancy.restoreProject': () => ({ project: PROJECT }),
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
  };
}
