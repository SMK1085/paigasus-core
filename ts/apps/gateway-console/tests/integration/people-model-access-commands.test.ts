// SPDX-License-Identifier: Apache-2.0
//
// The two commands of "Model access for people" (SMA-676 spec § 4.4, § 5.2, § 5.3, § 6) against the
// fake IAM. The role key is a server constant. The revoke is bounded to gateway_user of THAT
// principal at THIS org: a crafted form cannot revoke another role (§ 6), also against an old IAM
// that ignores the new filters (§ 10).
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { organizationPrn } from '@paigasus/console-core';
import { denial, startFakeIam, type FakeIam } from '@paigasus/console-core/testing';
import { grantModelAccess, revokeModelAccess } from '../../app/(console)/people-model-access/commands';
import { IDS, callsSince, clientsFor } from './support';

const ORG = organizationPrn(IDS.orgA);
const PERSON = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000c1';
const GRANT_ID = '0190a1d4-0000-7000-8000-0000000000c2';
const ADMIN_GRANT_ID = '0190a1d4-0000-7000-8000-0000000000c3';

let iam: FakeIam;
beforeAll(async () => {
  iam = await startFakeIam();
});
afterAll(async () => {
  disposeTransports();
  await iam.close();
});

describe('grantModelAccess (§ 5.2)', () => {
  it('grants gateway_user to the person at the org', async () => {
    iam.setHandlers({ 'authz.grantRole': (req) => ({ grant: { id: GRANT_ID, principalPrn: req.principalPrn, roleKey: req.roleKey, scopePrn: req.scopePrn } }) });
    const calls = callsSince(iam);
    expect(await grantModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG })).toEqual({ ok: true });
    expect(calls('authz.grantRole').map((call) => call.request)).toEqual([expect.objectContaining({ principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG })]);
  });

  it('returns a denial as IAM answers it', async () => {
    iam.setHandlers({
      'authz.grantRole': () => {
        throw denial();
      },
    });
    const result = await grantModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG });
    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
  });
});

describe('revokeModelAccess (§ 5.3)', () => {
  it('checks the grant first, then revokes it', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }),
      'authz.revokeRole': () => ({}),
    });
    const calls = callsSince(iam);
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toEqual({ ok: true });
    expect(calls('authz.listRoleGrants')[0]?.request).toMatchObject({ principalPrn: PERSON, scopePrn: ORG, roleKey: 'gateway_user' });
    expect(calls('authz.revokeRole').map((call) => call.request)).toEqual([expect.objectContaining({ id: GRANT_ID })]);
  });

  it('refuses a grant id that is not this person’s gateway_user grant at this org, and revokes nothing (§ 4.4, § 6)', async () => {
    iam.setHandlers({ 'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }) });
    const calls = callsSince(iam);
    const result = await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: ADMIN_GRANT_ID });
    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  it('refuses a grant id of another role even when IAM ignores the filters (spec § 10)', async () => {
    // An old IAM answers every grant of the principal: the org_admin grant is in the answer too.
    iam.setHandlers({
      'authz.listRoleGrants': () => ({
        grants: [
          { id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG },
          { id: ADMIN_GRANT_ID, principalPrn: PERSON, roleKey: 'org_admin', scopePrn: ORG },
        ],
      }),
    });
    const calls = callsSince(iam);
    const result = await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: ADMIN_GRANT_ID });
    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  it('is a success with no revoke when the grant is already gone (§ 5.3 step 4)', async () => {
    iam.setHandlers({ 'authz.listRoleGrants': () => ({ grants: [] }) });
    const calls = callsSince(iam);
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toEqual({ ok: true });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  it('is a success when another admin revoked between the check and the revoke (not-found)', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }),
      'authz.revokeRole': () => {
        throw denial({ code: Code.NotFound, reason: 'not-found' });
      },
    });
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toEqual({ ok: true });
  });

  it('returns a denial from the check', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => {
        throw denial();
      },
    });
    const calls = callsSince(iam);
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(calls('authz.revokeRole')).toHaveLength(0);
  });

  // F10: a test titled after a revoke denial must make revokeRole itself deny, not only the
  // bound-check list, and assert the denied result.
  it('returns a denial from the revoke', async () => {
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }),
      'authz.revokeRole': () => {
        throw denial();
      },
    });
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: ORG, grantId: GRANT_ID })).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
  });

  // Controller ruling (canonical PRN): IAM answers with canonical PRNs; the form is client text. A
  // form orgPrn that names the SAME organization, but with the UUID in a different case, must bind
  // exactly as the canonical form does — the check is on organization identity, not on the byte
  // form of the PRN string. Chosen result: success, the same as with the canonical-case orgPrn.
  it('binds on organization identity, not on the exact case of the form’s org PRN UUID (§ 6)', async () => {
    const differentCaseOrgPrn = ORG.replace(IDS.orgA, IDS.orgA.toUpperCase());
    iam.setHandlers({
      'authz.listRoleGrants': () => ({ grants: [{ id: GRANT_ID, principalPrn: PERSON, roleKey: 'gateway_user', scopePrn: ORG }] }),
      'authz.revokeRole': () => ({}),
    });
    const calls = callsSince(iam);
    expect(await revokeModelAccess({ authz: clientsFor(iam).authz }, { principalPrn: PERSON, orgPrn: differentCaseOrgPrn, grantId: GRANT_ID })).toEqual({ ok: true });
    expect(calls('authz.revokeRole').map((call) => call.request)).toEqual([expect.objectContaining({ id: GRANT_ID })]);
  });
});
