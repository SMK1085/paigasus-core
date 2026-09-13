// SPDX-License-Identifier: Apache-2.0
//
// What each Server Action revalidates (review, defect 4).
//
// THE DEFECT THIS PINS. `tests/support/next-cache.ts` recorded only the PATH, and `revalidatedPaths`
// was asserted by no test at all — so the second argument, `'layout'`, was covered by nothing below
// the e2e tier. The scope is not decoration: `revalidatePath('/orgs', 'page')` refreshes the one
// route, while `'layout'` refreshes every page under `/orgs`, which is what makes a created team
// appear in the list its parent layout renders. A regression to `'page'`, or a dropped call, now
// reds here.
//
// These tests drive the ACTION, not the command: the command tests in tests/integration/ cover the
// IAM call, and the action shell is the only place `revalidatePath` is named.
import { describe, expect, it, vi } from 'vitest';
import { Code, ConnectError } from '@connectrpc/connect';
import { revalidatedPaths } from '../support/next-cache';

const { tenancy, failNext } = vi.hoisted(() => {
  let failing = false;
  const call = (): Promise<Record<string, never>> => {
    if (!failing) return Promise.resolve({});
    failing = false;
    return Promise.reject(new ConnectError('nope', Code.PermissionDenied));
  };
  return {
    failNext: (): void => {
      failing = true;
    },
    tenancy: { createOrganization: call, attachMembership: call, detachMembership: call, createTeam: call, createProject: call },
  };
});

// The session read is the one thing an action does before its command, and it needs a Next request
// scope. tests/unit/action-session.test.ts covers that read itself; here it always succeeds.
vi.mock('../../lib/iam', () => ({ iamClientsForAction: () => Promise.resolve({ ok: true, value: { tenancy } }) }));

const { attachMembershipAction, createOrganizationAction, detachMembershipAction } = await import('../../app/(console)/orgs/actions');
const { createTeamAction } = await import('../../app/(console)/orgs/[org]/actions');
const { createProjectAction } = await import('../../app/(console)/orgs/[org]/teams/[team]/actions');

const ORG_PRN = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1';
const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';

function form(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [name, value] of Object.entries(fields)) data.append(name, value);
  return data;
}

/** Every action, with a form its zod schema accepts. The paths are basePath-RELATIVE. */
const ACTIONS = [
  ['createOrganizationAction', () => createOrganizationAction(null, form({ slug: 'acme', name: 'Acme' }))],
  ['attachMembershipAction', () => attachMembershipAction(null, form({ principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN }))],
  ['detachMembershipAction', () => detachMembershipAction(null, form({ id: '0190a1d4-0000-7000-8000-0000000000c1' }))],
  ['createTeamAction', () => createTeamAction(null, form({ orgPrn: ORG_PRN, slug: 'platform', name: 'Platform' }))],
  ['createProjectAction', () => createProjectAction(null, form({ teamPrn: TEAM_PRN, slug: 'api', name: 'API' }))],
] as const;

describe('every Server Action revalidates /orgs with the layout scope', () => {
  it.each(ACTIONS)('%s', async (_name, run) => {
    const result = await run();

    expect(result).toEqual({ ok: true });
    // Strict equality on BOTH arguments: 'layout' is what refreshes the list the parent layout
    // renders, and 'page' would leave a created node invisible until a manual reload.
    expect(revalidatedPaths).toEqual([{ path: '/orgs', type: 'layout' }]);
  });

  // The other direction: the call is inside `if (result.ok)`, so a refused action must revalidate
  // NOTHING. Without this, an action that revalidated unconditionally would still pass above.
  it.each(ACTIONS)('%s revalidates nothing when IAM refuses', async (_name, run) => {
    failNext();

    const result = await run();

    expect(result).toMatchObject({ ok: false });
    expect(revalidatedPaths).toEqual([]);
  });

  // A form zod refuses never reaches IAM, so it revalidates nothing either.
  it('revalidates nothing when the form is invalid', async () => {
    const result = await createOrganizationAction(null, form({ slug: '', name: 'Acme' }));

    expect(result).toMatchObject({ ok: false });
    expect(revalidatedPaths).toEqual([]);
  });
});
