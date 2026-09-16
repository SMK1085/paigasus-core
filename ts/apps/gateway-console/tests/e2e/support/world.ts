// SPDX-License-Identifier: Apache-2.0
//
// The IAM and gateway state the e2e tier scripts (spec § 10.4). This zone calls five IAM methods —
// authn.introspect, authz.listRoleGrants, tenancy.getOrganization, tenancy.getTeam,
// tenancy.getProject — and `setHandlers()` REPLACES the whole map, so the default set below must
// carry every key an override can use. An unused handler is dead code a reviewer will flag: the
// default world gives the principal a team membership and a project-scoped grant (besides the
// organization membership), so every default login actually walks all three tenancy lookups through
// @paigasus/console-core's myScopes(), not only the org one — even though no spec here asserts on
// the team/project results directly (this zone has no team or project screen, spec § 13).
//
// PRNs are literal strings: @paigasus/console-core's prn-tenancy.ts imports server-only, which
// throws under Playwright.
import { Code } from '@connectrpc/connect';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';

export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const ORG_NAME = 'Acme Research';

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_IAM_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar'] };
export const DEFAULT_GATEWAY_DESCRIPTOR: Descriptor = { service: 'gateway', version: '0.0.0-e2e', capabilities: ['gateway.chat.stream'] };

export type WorldOptions = {
  /** false: a first-time identity with no membership and no grant. Default: true. */
  readonly memberships?: boolean;
  /** What the fake IAM's `GET /v1/service-info` answers. Default: DEFAULT_IAM_DESCRIPTOR. */
  readonly iamDescriptor?: Descriptor;
  /** What the fake gateway's `GET /v1/service-info` answers. Default: DEFAULT_GATEWAY_DESCRIPTOR. */
  readonly gatewayDescriptor?: Descriptor;
  /** Replace single handlers, for example with one that throws denial(). */
  readonly overrides?: FakeIamHandlers;
};

const TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e2';
const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
const PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e3';
const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;

const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: 'Platform Team' };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: 'Inference Gateway' };

const notFound = (): Error => denial({ code: Code.NotFound, reason: 'not-found' });

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const withScopes = options.memberships ?? true;
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
    'authz.listRoleGrants': () => ({
      grants: withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: PROJECT_PRN }] : [],
    }),
    'tenancy.getOrganization': (req: { prn: string }) => {
      if (req.prn !== ORG_PRN) throw notFound();
      return { organization: ORGANIZATION };
    },
    'tenancy.getTeam': (req: { prn: string }) => {
      if (req.prn !== TEAM_PRN) throw notFound();
      return { team: TEAM };
    },
    'tenancy.getProject': (req: { prn: string }) => {
      if (req.prn !== PROJECT_PRN) throw notFound();
      return { project: PROJECT };
    },
    ...options.overrides,
  };
}
