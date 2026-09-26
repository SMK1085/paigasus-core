// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 spec § 7.3 (D12): the per-request IAM call count of /gateway/orgs/<org>. The page is
// fetched with a PLAIN request (page.request: the context's cookies, no router, so no prefetch).
//
// F8: proxy.ts mints one correlation id per request, into the REQUEST headers only — it sets
// nothing on the RESPONSE (measured: proxy.ts's `proceed` carries no CORRELATION_HEADER write, and
// `authMiddleware`'s decision sets none today either), so the plain fetch response carries no
// correlation id to read back. The fake's call log is therefore grouped by the id it saw on each
// call (fake-iam.ts records the raw incoming `paigasus-correlation-id` header), and the ONE group
// that holds ListServiceAccounts is this render.
//
// The expected counts are the § 7.3 formula, derived from § 4.2 BEFORE the measurement. A
// difference is a finding to explain, not a number to copy in here. Only gRPC calls are counted:
// the discovery probe is HTTP (`http.getServiceInfo`), is not in the formula, and is memoized by
// the descriptor cache, not per request.
//
// LIMIT (§ 7.3): this counts calls against a fake. It does not measure latency against a real IAM.
import type { FakeIamCall } from '@paigasus/console-core/testing';
import { signIn } from './support/login';
import { ORG_ID, SEEDED_SA_ID } from './support/world';
import { expect, test } from './support/harness';

const ORG_PATH = `/gateway/orgs/${ORG_ID}`;

/** The default world's scopes (support/world.ts): an organization and a team membership, and a project grant. */
const SCOPES = { organization: 1, team: 1, project: 1 } as const;
/** The default world's teams in the organization (support/world.ts `tenancy.listTeams`). */
const TEAMS = 1;

/** § 7.3's table for S scopes and T teams, with no `sa` parameter. SMA-676 adds the people section. */
function formula(): Record<string, number> {
  return {
    'authn.whoAmI': 1, // the session principal, memoized per request
    'authz.listRoleGrants': 2, // myScopes(), with iam.authz.cedar present; and the people section's user grants at the org (SMA-676): one short page, no probe
    'tenancy.getOrganization': 1 + SCOPES.organization, // the page, plus myScopes()'s label of the org scope
    'tenancy.getTeam': SCOPES.team, // myScopes() labels
    'tenancy.getProject': SCOPES.project, // myScopes() labels
    'authz.isAuthorized': 6, // mayI(), six distinct questions: the five of SMA-636, and RevokeRole at the org (SMA-676); GrantRole at the org is memoized
    'serviceAccounts.listServiceAccounts': 1, // the section
    'tenancy.listTeams': 1, // the Projects list
    'tenancy.listProjects': TEAMS, // the Projects list, one per shown team
    'tenancy.listMemberships': 1, // the people section's user members of the org (SMA-676): one short page, no probe
  };
}

/** The gRPC calls of the ONE request whose group holds ListServiceAccounts, counted by method. */
function countOneRender(calls: readonly FakeIamCall[]): Record<string, number> {
  const groups = new Map<string, FakeIamCall[]>();
  for (const call of calls) {
    if (call.correlationId === null || call.method.startsWith('http.')) continue;
    groups.set(call.correlationId, [...(groups.get(call.correlationId) ?? []), call]);
  }
  const renders = [...groups.values()].filter((group) => group.some((call) => call.method === 'serviceAccounts.listServiceAccounts'));
  expect(renders).toHaveLength(1);
  const counts: Record<string, number> = {};
  for (const call of renders[0] ?? []) counts[call.method] = (counts[call.method] ?? 0) + 1;
  return counts;
}

test('R19: one render of the organization page makes exactly the calls of the § 7.3 formula (D12)', async ({ page, harness }) => {
  await signIn(page, harness, '/gateway/overview');
  const start = harness.iam.calls.length;

  const response = await page.request.get(harness.url(ORG_PATH));

  expect(response.status()).toBe(200);
  expect(countOneRender(harness.iam.calls.slice(start))).toEqual(formula());
});

test('R20: with ?sa=, the same render adds exactly one GetServiceAccount, one IsAuthorized and one ListApiKeys (§ 7.3)', async ({ page, harness }) => {
  harness.useWorld({ seedServiceAccount: true });
  await signIn(page, harness, '/gateway/overview');
  const start = harness.iam.calls.length;

  const response = await page.request.get(harness.url(`${ORG_PATH}?sa=${SEEDED_SA_ID}`));

  expect(response.status()).toBe(200);
  const base = formula();
  expect(countOneRender(harness.iam.calls.slice(start))).toEqual({
    ...base,
    'authz.isAuthorized': (base['authz.isAuthorized'] ?? 0) + 1, // the model-call state
    'serviceAccounts.getServiceAccount': 1,
    'serviceAccounts.listApiKeys': 1,
  });
});
