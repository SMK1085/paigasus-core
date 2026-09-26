// SPDX-License-Identifier: Apache-2.0
//
// The IAM and gateway state the e2e tier scripts (SMA-512 spec § 10.4; SMA-636 spec § 7.2).
// `setHandlers()` REPLACES the whole map, so worldHandlers() always returns the FULL set.
//
// STATEFUL since SMA-636. One worldHandlers() call is one world: it records the service accounts,
// the keys and the grants that the create, issue, revoke, archive and grant calls make, and every
// later answer of the SAME world reads them. harness.useWorld() starts a new world, so a test never
// sees another test's accounts. IsAuthorized about a SERVICE ACCOUNT answers from the recorded
// gateway_user grants, so the main row cannot show "Can call models: Yes" without a grant.
// IsAuthorized about the signed-in USER answers from an allow list, as in iam-console's world.
// A second grant of the same role at the same scope returns the existing grant (SMA-676 D9). With
// orgCreator, IsAuthorized(InvokeModel) about the user reads the grants too (SMA-676 § 7.3).
//
// The two-zone tier serves the iam-console app from this same world, so the handlers that app's
// organization page needs (listTeams, listMemberships) are here too.
//
// PRNs are literal strings: @paigasus/console-core's prn-tenancy.ts imports server-only, which
// throws under Playwright. NodeStatus and ApiKeyStatus come from @paigasus/sdk's guard-free
// ./iam/types entry.
import { randomUUID } from 'node:crypto';
import { Code, ConnectError } from '@connectrpc/connect';
import { denial, type FakeIamHandlers } from '@paigasus/console-core/testing';
import { ApiKeyStatus, NodeStatus, PrincipalKind } from '@paigasus/sdk/iam/types';

export const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';
export const ORG_ID = '0190a100-0000-7000-8000-0000000000e1';
export const ORG_PRN = `prn:pgs:iam:::organization/${ORG_ID}`;
export const ORG_NAME = 'Acme Research';
export const TEAM_ID = '0190a1b2-0000-7000-8000-0000000000e2';
export const TEAM_PRN = `prn:pgs:iam::${ORG_ID}:team/${TEAM_ID}`;
export const TEAM_NAME = 'Platform Team';
export const PROJECT_ID = '0190a1c3-0000-7000-8000-0000000000e3';
export const PROJECT_PRN = `prn:pgs:iam::${ORG_ID}:project/${PROJECT_ID}`;
export const PROJECT_NAME = 'Inference Gateway';

/** The account `seedServiceAccount` puts into a world: owned by the organization, active, no key, no grant. */
export const SEEDED_SA_ID = '0190a1e5-0000-7000-8000-0000000000a1';
export const SEEDED_SA_PRN = `prn:pgs:iam:::principal/${SEEDED_SA_ID}`;
export const SEEDED_SA_NAME = 'seeded-bot';

/** Every token this world issues is this prefix and then 32 hex digits. */
export const TOKEN_PREFIX = 'pgs_e2e_';

export type Descriptor = { service: string; version: string; capabilities: string[] } | { status: number };
export const DEFAULT_IAM_DESCRIPTOR: Descriptor = { service: 'iam', version: '0.0.0-e2e', capabilities: ['iam.authz.cedar', 'iam.apikeys'] };
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
  /** The actions IsAuthorized allows the SIGNED-IN USER. Default: every action. */
  readonly allow?: readonly string[];
  /** true: the FIRST GrantRole fails with an internal error; later ones succeed (§ 7.2 row 2). */
  readonly failFirstGrant?: boolean;
  /** true: the world starts with SEEDED_SA_*, owned by the organization. */
  readonly seedServiceAccount?: boolean;
  /** true: the user is project_admin of PROJECT only — no membership, and no organization access (§ 7.2 row 5). */
  readonly projectAdmin?: boolean;
  /**
   * SMA-676 § 7.3: the user CREATED the organization. It holds org_admin at ORG_PRN and is not an
   * org member (§ 1.1 fact 6), and IsAuthorized(InvokeModel) about the user answers from the
   * recorded gateway_user grants, as the service-account branch does. Off by default, so R24 and
   * R25 keep a user whose InvokeModel is allowed with no grant.
   */
  readonly orgCreator?: boolean;
};

const ACTIVE = { status: NodeStatus.ACTIVE, effectiveStatus: NodeStatus.ACTIVE };
const CREATED = { createdAt: { seconds: 1_788_000_000n, nanos: 0 } };

const ORGANIZATION = { prn: ORG_PRN, slug: 'acme', name: ORG_NAME, ...ACTIVE };
const TEAM = { prn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'platform', name: TEAM_NAME, ...ACTIVE };
const PROJECT = { prn: PROJECT_PRN, teamPrn: TEAM_PRN, orgPrn: ORG_PRN, slug: 'gateway', name: PROJECT_NAME, ...ACTIVE };

type Account = { readonly prn: string; readonly ownerPrn: string; readonly name: string; status: string };
type Key = {
  readonly id: string;
  readonly serviceAccountPrn: string;
  readonly scopePrn: string;
  readonly prefix: string;
  status: ApiKeyStatus;
  readonly expiresAt: { readonly seconds: bigint; readonly nanos: number } | undefined;
};
type Grant = { readonly id: string; readonly principalPrn: string; readonly roleKey: string; readonly scopePrn: string };

const notFound = (): Error => denial({ code: Code.NotFound, reason: 'not-found' });

/** A grant at `scopePrn` covers `resourcePrn` when it is the same node or its organization (Cedar's `resource in ?resource`). */
function covers(scopePrn: string, resourcePrn: string): boolean {
  return scopePrn === resourcePrn || (scopePrn === ORG_PRN && resourcePrn.startsWith(`prn:pgs:iam::${ORG_ID}:`));
}

/** IAM's offset paging: limit 0 is the server default of 50. */
function page<T>(items: readonly T[], request: { readonly limit: number; readonly offset: bigint }): T[] {
  const start = Number(request.offset);
  return items.slice(start, start + (request.limit === 0 ? 50 : request.limit));
}

/** The signed-in user's OWN role grants (myScopes() lists them). */
function userGrants(projectAdmin: boolean, withScopes: boolean, orgCreator: boolean): Grant[] {
  if (projectAdmin) return [{ id: '0190a1d4-0000-7000-8000-0000000000f6', principalPrn: PRINCIPAL_PRN, roleKey: 'project_admin', scopePrn: PROJECT_PRN }];
  const own: Grant[] = withScopes ? [{ id: '0190a1d4-0000-7000-8000-0000000000f3', principalPrn: PRINCIPAL_PRN, roleKey: 'project_viewer', scopePrn: PROJECT_PRN }] : [];
  if (orgCreator) own.push({ id: '0190a1d4-0000-7000-8000-0000000000f7', principalPrn: PRINCIPAL_PRN, roleKey: 'org_admin', scopePrn: ORG_PRN });
  return own;
}

export function worldHandlers(options: WorldOptions = {}): FakeIamHandlers {
  const projectAdmin = options.projectAdmin === true;
  const orgCreator = options.orgCreator === true;
  // F9: the org creator holds org_admin but is not a member (§ 1.1 fact 6), so whoAmI reports NO
  // org/team membership for it.
  const withScopes = (options.memberships ?? true) && !projectAdmin && !orgCreator;
  const allow = options.allow === undefined ? null : new Set(options.allow);
  const accounts: Account[] = options.seedServiceAccount === true ? [{ prn: SEEDED_SA_PRN, ownerPrn: ORG_PRN, name: SEEDED_SA_NAME, status: 'active' }] : [];
  const keys: Key[] = [];
  const grants: Grant[] = [];
  let grantFailuresLeft = options.failFirstGrant === true ? 1 : 0;

  const accountView = (account: Account) => ({ prn: account.prn, ownerPrn: account.ownerPrn, name: account.name, status: account.status, audit: CREATED });
  const keyView = (key: Key) => ({
    id: key.id,
    serviceAccountPrn: key.serviceAccountPrn,
    scopePrn: key.scopePrn,
    prefix: key.prefix,
    status: key.status,
    ...(key.expiresAt === undefined ? {} : { expiresAt: { seconds: key.expiresAt.seconds, nanos: key.expiresAt.nanos } }),
    audit: CREATED,
  });
  const accountAt = (prn: string): Account => {
    const account = accounts.find((candidate) => candidate.prn === prn);
    if (account === undefined) throw notFound();
    return account;
  };
  /** A call a project_admin may not make: it reads the organization or the team (§ 7.2 row 5). */
  const organizationOnly = (): void => {
    if (projectAdmin) throw denial();
  };
  /** A principal is a service account when the world made it one; every other principal is a user. */
  const isServiceAccount = (prn: string): boolean => accounts.some((account) => account.prn === prn);
  const ofKind = (prn: string, kind: PrincipalKind): boolean => kind === PrincipalKind.UNSPECIFIED || (kind === PrincipalKind.SERVICE_ACCOUNT) === isServiceAccount(prn);
  const grantsAt = (): Grant[] => [...userGrants(projectAdmin, withScopes, orgCreator), ...grants];

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
    // The bare principal request (myScopes) keeps its old answer. A request with a scope, a role or a
    // kind is IAM's query path (SMA-676 D2, D6): exact scope match, principal order, IAM's paging.
    'authz.listRoleGrants': (req) => {
      const query = req.scopePrn !== '' || req.roleKey !== '' || req.principalKind !== PrincipalKind.UNSPECIFIED;
      if (!query) return { grants: userGrants(projectAdmin, withScopes, orgCreator) };
      const hits = grantsAt()
        .filter(
          (grant) =>
            (req.principalPrn === '' || grant.principalPrn === req.principalPrn) &&
            (req.scopePrn === '' || grant.scopePrn === req.scopePrn) &&
            (req.roleKey === '' || grant.roleKey === req.roleKey) &&
            ofKind(grant.principalPrn, req.principalKind),
        )
        .sort((a, b) => (a.principalPrn < b.principalPrn ? -1 : a.principalPrn > b.principalPrn ? 1 : a.id < b.id ? -1 : 1));
      return { grants: page(hits, req) };
    },
    'authz.isAuthorized': (req) => {
      if (req.principalPrn === PRINCIPAL_PRN) {
        // SMA-676 § 7.3: the org creator's InvokeModel answers from the recorded grants.
        if (orgCreator && req.action === 'InvokeModel') {
          const allowed = grants.some((grant) => grant.principalPrn === PRINCIPAL_PRN && grant.roleKey === 'gateway_user' && covers(grant.scopePrn, req.resourcePrn));
          return { allowed, determiningPolicies: [], reason: '' };
        }
        return { allowed: allow === null || allow.has(req.action), determiningPolicies: [], reason: '' };
      }
      // About a service account: only InvokeModel, and only from a recorded gateway_user grant.
      const allowed = req.action === 'InvokeModel' && grants.some((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === 'gateway_user' && covers(grant.scopePrn, req.resourcePrn));
      return { allowed, determiningPolicies: [], reason: '' };
    },
    'authz.grantRole': (req) => {
      if (grantFailuresLeft > 0) {
        grantFailuresLeft -= 1;
        throw new ConnectError('the e2e world fails this grant', Code.Internal);
      }
      // SMA-676 D9: a second grant of the same role at the same scope returns the existing grant.
      const existing = grants.find((grant) => grant.principalPrn === req.principalPrn && grant.roleKey === req.roleKey && grant.scopePrn === req.scopePrn);
      if (existing !== undefined) return { grant: existing };
      const grant: Grant = { id: randomUUID(), principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn };
      grants.push(grant);
      return { grant };
    },
    'authz.revokeRole': (req) => {
      const index = grants.findIndex((grant) => grant.id === req.id);
      if (index === -1) throw notFound();
      grants.splice(index, 1);
      return {};
    },
    'tenancy.getOrganization': (req) => {
      organizationOnly();
      if (req.prn !== ORG_PRN) throw notFound();
      return { organization: ORGANIZATION };
    },
    'tenancy.getTeam': (req) => {
      organizationOnly();
      if (req.prn !== TEAM_PRN) throw notFound();
      return { team: TEAM };
    },
    'tenancy.getProject': (req) => {
      if (req.prn !== PROJECT_PRN) throw notFound();
      return { project: PROJECT };
    },
    'tenancy.listTeams': (req) => {
      organizationOnly();
      return { teams: req.orgPrn === ORG_PRN ? [TEAM] : [] };
    },
    'tenancy.listProjects': (req) => ({ projects: req.teamPrn === TEAM_PRN ? [PROJECT] : [] }),
    // `filter` is a oneof: its type includes `{ case: undefined }`, so narrow instead of annotating.
    // SMA-676: the org creator is no member of the org; the kind filter keeps only users.
    'tenancy.listMemberships': (req) => {
      const nodePrn = req.filter.case === 'nodePrn' ? req.filter.value : '';
      if (orgCreator && nodePrn === ORG_PRN) return { memberships: [] };
      if (!ofKind(PRINCIPAL_PRN, req.principalKind)) return { memberships: [] };
      return { memberships: [{ id: '0190a1d4-0000-7000-8000-0000000000f4', principalPrn: PRINCIPAL_PRN, nodePrn }] };
    },
    'serviceAccounts.createServiceAccount': (req) => {
      const name = req.name.trim();
      if (accounts.some((account) => account.ownerPrn === req.ownerPrn && account.name === name)) throw denial({ code: Code.AlreadyExists, reason: 'service-account-name-conflict' });
      const account: Account = { prn: `prn:pgs:iam:::principal/${randomUUID()}`, ownerPrn: req.ownerPrn, name, status: 'active' };
      accounts.push(account);
      return { serviceAccount: accountView(account) };
    },
    'serviceAccounts.getServiceAccount': (req) => ({ serviceAccount: accountView(accountAt(req.prn)) }),
    'serviceAccounts.listServiceAccounts': (req) => ({
      serviceAccounts: page(
        accounts.filter((account) => account.ownerPrn === req.ownerPrn),
        req,
      ).map(accountView),
    }),
    // IAM disables the principal; its key rows stay ACTIVE (spec § 3.1). The console shows them inactive.
    'serviceAccounts.archiveServiceAccount': (req) => {
      accountAt(req.prn).status = 'disabled';
      return {};
    },
    'serviceAccounts.issueApiKey': (req) => {
      accountAt(req.serviceAccountPrn);
      const token = `${TOKEN_PREFIX}${randomUUID().replaceAll('-', '')}`;
      const key: Key = {
        id: randomUUID(),
        serviceAccountPrn: req.serviceAccountPrn,
        scopePrn: req.scopePrn,
        prefix: token.slice(0, 12),
        status: ApiKeyStatus.ACTIVE,
        expiresAt: req.expiresAt === undefined ? undefined : { seconds: req.expiresAt.seconds, nanos: req.expiresAt.nanos },
      };
      keys.push(key);
      return { apiKey: keyView(key), token };
    },
    'serviceAccounts.revokeApiKey': (req) => {
      const key = keys.find((candidate) => candidate.id === req.id);
      if (key === undefined) throw notFound();
      key.status = ApiKeyStatus.REVOKED;
      return {};
    },
    'serviceAccounts.listApiKeys': (req) => ({
      apiKeys: page(
        keys.filter((key) => key.serviceAccountPrn === req.serviceAccountPrn),
        req,
      ).map(keyView),
    }),
    ...options.overrides,
  };
}
