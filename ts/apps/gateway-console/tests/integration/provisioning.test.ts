// SPDX-License-Identifier: Apache-2.0
//
// What is specific to THIS app's wiring — lib/auth.ts's resolver and lib/console.ts's myScopes() —
// rather than to @paigasus/console-core, which has its own tier for loadMyScopes/the resolver
// factory itself (their own unit/integration suites). This file asserts the ASSEMBLY: that this
// app's resolver calls the two IAM RPCs in the right ORDER and with the request's correlation id,
// and that myScopes() through this app's real runtime picks memberships-only vs. memberships+grants
// off the real discovery() probe.
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from 'vitest';
import { organizationPrn, resetDiscoveryForTest, SCOPE_CAP } from '@paigasus/console-core';
import { authRuntime } from '../../lib/auth';
import { myScopes } from '../../lib/console';
import { setRequestHeaders } from '../support/next-headers';
import { IDS, installSession, startIntegrationEnv, stopIntegrationEnv, type IntegrationEnv } from './support';

let env: IntegrationEnv;

beforeAll(async () => {
  env = await startIntegrationEnv();
});

afterAll(async () => {
  await stopIntegrationEnv(env);
});

beforeEach(() => {
  env.iam.setServiceInfo({ service: 'iam', version: '0.0.0-fake', capabilities: ['iam.audit'] });
  env.iam.setHandlers({ 'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'org', name: `Org ${req.prn.slice(-4)}` } }) });
});

afterEach(() => {
  resetDiscoveryForTest();
});

/** A unique, valid org UUID, distinct per index. */
function orgId(index: number): string {
  return `0190a300-0000-7000-8000-${index.toString(16).padStart(12, '0')}`;
}

describe("this app's resolver (lib/auth.ts)", () => {
  it('calls GetServiceInfo BEFORE Introspect, both carrying the request correlation id', async () => {
    const correlationId = '0198f2c1-8888-7000-8000-0000000000aa';
    setRequestHeaders({ 'paigasus-correlation-id': correlationId });
    const runtime = await authRuntime();
    const before = env.iam.calls.length;

    await runtime.resolver.resolve({ accessToken: 'tok-resolver-order', idTokenClaims: { iss: 'https://idp.example.test', sub: 'resolver-subject' } });

    const calls = env.iam.calls.slice(before);
    const serviceInfoIndex = calls.findIndex((call) => call.method === 'serviceInfo.getServiceInfo');
    const introspectIndex = calls.findIndex((call) => call.method === 'authn.introspect');

    expect(serviceInfoIndex).toBeGreaterThanOrEqual(0);
    expect(introspectIndex).toBeGreaterThan(serviceInfoIndex);
    expect(calls[serviceInfoIndex]?.correlationId).toBe(correlationId);
    expect(calls[introspectIndex]?.correlationId).toBe(correlationId);
  });
});

describe("myScopes() through this app's runtime", () => {
  it('returns memberships only when IAM reports no iam.authz.cedar', async () => {
    env.iam.setServiceInfo({ service: 'iam', version: '0.0.0-fake', capabilities: ['iam.audit'] });
    env.iam.setHandlers({
      'authn.introspect': () => ({ memberships: [{ id: 'm1', principalPrn: 'prn:pgs:iam:::principal/p1', nodePrn: organizationPrn(IDS.orgA) }] }),
      'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'org', name: 'Org A' } }),
    });
    await installSession('tok-scopes-no-cedar');

    const result = await myScopes();

    if (!result.ok) throw new Error(`expected ok, got ${result.error.presentation}`);
    expect(result.value.grantsListed).toBe(false);
    expect(result.value.entries.map((e) => e.orgId)).toEqual([IDS.orgA]);
    expect(env.iam.callsTo('authz.listRoleGrants')).toHaveLength(0);
  });

  it('returns memberships plus the principal’s own grants, DEDUPLICATED, when IAM reports iam.authz.cedar', async () => {
    env.iam.setServiceInfo({ service: 'iam', version: '0.0.0-fake', capabilities: ['iam.authz.cedar'] });
    env.iam.setHandlers({
      'authn.introspect': () => ({ memberships: [{ id: 'm1', principalPrn: 'prn:pgs:iam:::principal/p1', nodePrn: organizationPrn(IDS.orgA) }] }),
      // orgA is a DUPLICATE of the membership above; orgB is new. The result must list each org once.
      'authz.listRoleGrants': () => ({
        grants: [
          { scopePrn: organizationPrn(IDS.orgA), roleKey: 'org_admin' },
          { scopePrn: organizationPrn(IDS.orgB), roleKey: 'org_admin' },
        ],
      }),
      'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'org', name: 'Org' } }),
    });
    await installSession('tok-scopes-cedar');

    const result = await myScopes();

    if (!result.ok) throw new Error(`expected ok, got ${result.error.presentation}`);
    expect(result.value.grantsListed).toBe(true);
    expect(result.value.entries).toHaveLength(2);
    expect(result.value.entries.map((e) => e.orgId).sort()).toEqual([IDS.orgA, IDS.orgB].sort());
    expect(env.iam.callsTo('authz.listRoleGrants').length).toBeGreaterThan(0);
  });

  it(`caps the listed scopes at SCOPE_CAP (${SCOPE_CAP}) and reports the rest as hidden`, async () => {
    const total = SCOPE_CAP + 10;
    const grants = Array.from({ length: total }, (_, i) => ({ scopePrn: organizationPrn(orgId(i)), roleKey: 'org_admin' }));
    env.iam.setServiceInfo({ service: 'iam', version: '0.0.0-fake', capabilities: ['iam.authz.cedar'] });
    env.iam.setHandlers({
      'authn.introspect': () => ({ memberships: [] }),
      'authz.listRoleGrants': () => ({ grants }),
      'tenancy.getOrganization': (req: { prn: string }) => ({ organization: { prn: req.prn, slug: 'org', name: 'Org' } }),
    });
    await installSession('tok-scopes-cap');

    const result = await myScopes();

    if (!result.ok) throw new Error(`expected ok, got ${result.error.presentation}`);
    expect(result.value.entries).toHaveLength(SCOPE_CAP);
    expect(result.value.hiddenCount).toBe(10);
  });
});
