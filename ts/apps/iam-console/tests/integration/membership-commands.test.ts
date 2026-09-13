// SPDX-License-Identifier: Apache-2.0
//
// Create team (spec § 5.3) and the two membership commands every node page shares. The node PRN
// and the membership id arrive as form fields; IAM checks them, so no command pre-judges.
import { ErrorReason } from '@paigasus/sdk/errors/types';
import { Code } from '@connectrpc/connect';
import { disposeTransports } from '@paigasus/sdk/iam';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createTeam, createTeamForm } from '../../app/(console)/orgs/[org]/commands';
import { attachMembership, attachMembershipForm, detachMembership, detachMembershipForm } from '../../app/(console)/orgs/commands';
import { organizationPrn, teamPrn } from '../../lib/prn-tenancy';
import { denial, startFakeIam, type FakeIam } from '../support/fake-iam';
import { IDS, callsSince, clientsFor } from './support';

let iam: FakeIam;

beforeAll(async () => {
  iam = await startFakeIam();
});

afterAll(async () => {
  disposeTransports();
  await iam.close();
});

const ORG_A = organizationPrn(IDS.orgA);
const TEAM_A1 = teamPrn(IDS.orgA, IDS.teamA1);

describe('createTeam', () => {
  it('creates a team under the organization from the hidden field', async () => {
    iam.setHandlers({ 'tenancy.createTeam': (req: { orgPrn: string; slug: string; name: string }) => ({ team: { prn: TEAM_A1, orgPrn: req.orgPrn, slug: req.slug, name: req.name } }) });
    const calls = callsSince(iam);

    expect(await createTeam({ tenancy: clientsFor(iam).tenancy }, { orgPrn: ORG_A, slug: 'platform', name: 'Platform' })).toEqual({ ok: true });
    expect(calls('tenancy.createTeam').map((call) => call.request)).toEqual([expect.objectContaining({ orgPrn: ORG_A, slug: 'platform', name: 'Platform' })]);
  });

  it("returns IAM's 403 with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.createTeam': () => {
        throw denial({ correlationId: 'corr-create-team' });
      },
    });

    const result = await createTeam({ tenancy: clientsFor(iam).tenancy }, { orgPrn: ORG_A, slug: 'platform', name: 'Platform' });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.correlationId).toBe('corr-create-team');
  });

  it('refuses a form without the parent PRN', () => {
    expect(createTeamForm.safeParse({ orgPrn: null, slug: 'a', name: 'A' }).success).toBe(false);
  });

  // Minor 11 of the final whole-branch review: orgPrn lacked the .trim() every other PRN field has,
  // so a padded hidden field reached IAM as a malformed PRN, and a whitespace-only one passed min(1).
  it('trims the parent PRN, and refuses one that is only whitespace', () => {
    expect(createTeamForm.safeParse({ orgPrn: `  ${ORG_A}\n`, slug: 'a', name: 'A' }).data?.orgPrn).toBe(ORG_A);
    expect(createTeamForm.safeParse({ orgPrn: '   ', slug: 'a', name: 'A' }).success).toBe(false);
  });
});

describe('attachMembership', () => {
  it('sends the typed principal PRN and the node PRN', async () => {
    iam.setHandlers({ 'tenancy.attachMembership': (req: { principalPrn: string; nodePrn: string }) => ({ membership: { id: IDS.membership, principalPrn: req.principalPrn, nodePrn: req.nodePrn } }) });
    const calls = callsSince(iam);

    expect(await attachMembership({ tenancy: clientsFor(iam).tenancy }, { principalPrn: IDS.principalPrn, nodePrn: ORG_A })).toEqual({ ok: true });
    expect(calls('tenancy.attachMembership').map((call) => call.request)).toEqual([expect.objectContaining({ principalPrn: IDS.principalPrn, nodePrn: ORG_A })]);
  });

  it('returns a duplicate membership as a conflict with its reason', async () => {
    iam.setHandlers({
      'tenancy.attachMembership': () => {
        throw denial({ code: Code.AlreadyExists, reason: 'duplicate-membership', correlationId: 'corr-dup' });
      },
    });

    const result = await attachMembership({ tenancy: clientsFor(iam).tenancy }, { principalPrn: IDS.principalPrn, nodePrn: ORG_A });

    if (result.ok) throw new Error('expected a conflict');
    expect(result.error.presentation).toBe('conflict');
    expect(result.error.reason).toBe(ErrorReason.DUPLICATE_MEMBERSHIP);
  });

  it('trims the principal PRN and refuses an empty one', () => {
    expect(attachMembershipForm.safeParse({ principalPrn: `  ${IDS.principalPrn} `, nodePrn: ORG_A }).data).toEqual({ principalPrn: IDS.principalPrn, nodePrn: ORG_A });
    expect(attachMembershipForm.safeParse({ principalPrn: ' ', nodePrn: ORG_A }).success).toBe(false);
  });
});

describe('detachMembership', () => {
  it('sends the membership id', async () => {
    iam.setHandlers({ 'tenancy.detachMembership': () => ({}) });
    const calls = callsSince(iam);

    expect(await detachMembership({ tenancy: clientsFor(iam).tenancy }, { id: IDS.membership })).toEqual({ ok: true });
    expect(calls('tenancy.detachMembership').map((call) => call.request)).toEqual([expect.objectContaining({ id: IDS.membership })]);
  });

  it("returns IAM's 403 with the correlation id", async () => {
    iam.setHandlers({
      'tenancy.detachMembership': () => {
        throw denial({ correlationId: 'corr-detach' });
      },
    });

    const result = await detachMembership({ tenancy: clientsFor(iam).tenancy }, { id: IDS.membership });

    if (result.ok) throw new Error('expected a denial');
    expect(result.error.presentation).toBe('forbidden');
    expect(result.error.correlationId).toBe('corr-detach');
  });

  it('refuses an empty id', () => {
    expect(detachMembershipForm.safeParse({ id: '' }).success).toBe(false);
  });
});
