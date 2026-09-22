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

const { tenancy, outbox, failNext } = vi.hoisted(() => {
  let failure: Code | null = null;
  const call = (): Promise<Record<string, never>> => {
    if (failure === null) return Promise.resolve({});
    const code = failure;
    failure = null;
    return Promise.reject(new ConnectError('nope', code));
  };
  return {
    failNext: (code: Code): void => {
      failure = code;
    },
    tenancy: {
      createOrganization: call,
      attachMembership: call,
      detachMembership: call,
      createTeam: call,
      createProject: call,
      renameOrganization: call,
      archiveOrganization: call,
      restoreOrganization: call,
      renameTeam: call,
      archiveTeam: call,
      restoreTeam: call,
      renameProject: call,
      archiveProject: call,
      restoreProject: call,
    },
    // SMA-629: recording mocks, so a test can count the calls an invalid id must NOT make. SMA-661
    // added bulk replay, which answers a count.
    outbox: {
      replayDeadLetter: vi.fn<(request: { id: string }) => Promise<Record<string, never>>>(call),
      discardDeadLetter: vi.fn<(request: { id: string }) => Promise<Record<string, never>>>(call),
      bulkReplayDeadLetters: vi.fn<(request: { eventType: string; maxRows: bigint }) => Promise<{ replayed: bigint }>>(() => call().then(() => ({ replayed: 3n }))),
    },
  };
});

// The session read is the one thing an action does before its command, and it needs a Next request
// scope. tests/unit/action-session.test.ts covers that read itself; here it always succeeds.
vi.mock('../../lib/console', () => ({ iamClientsForAction: () => Promise.resolve({ ok: true, value: { tenancy, outbox } }) }));

const { attachMembershipAction, createOrganizationAction, detachMembershipAction } = await import('../../app/(console)/orgs/actions');
const { archiveOrganizationAction, createTeamAction, renameOrganizationAction, restoreOrganizationAction } = await import('../../app/(console)/orgs/[org]/actions');
const { archiveTeamAction, createProjectAction, renameTeamAction, restoreTeamAction } = await import('../../app/(console)/orgs/[org]/teams/[team]/actions');
const { archiveProjectAction, renameProjectAction, restoreProjectAction } = await import('../../app/(console)/orgs/[org]/teams/[team]/projects/[project]/actions');
const { bulkReplayDeadLettersAction, discardDeadLetterAction, replayDeadLetterAction } = await import('../../app/(console)/dead-letters/actions');

const ORG_PRN = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1';
const PROJECT_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:project/0190a1c3-0000-7000-8000-0000000000a1';
const PRINCIPAL_PRN = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0';

function form(fields: Record<string, string>): FormData {
  const data = new FormData();
  for (const [name, value] of Object.entries(fields)) data.append(name, value);
  return data;
}

const rename = (prn: string): Record<string, string> => ({ prn, slug: 'new-slug', name: 'New Name', currentSlug: 'old-slug', currentName: 'Old Name' });

/** Every create and membership action, with a form its zod schema accepts. The paths are basePath-RELATIVE. */
const ACTIONS = [
  ['createOrganizationAction', () => createOrganizationAction(null, form({ slug: 'acme', name: 'Acme' }))],
  ['attachMembershipAction', () => attachMembershipAction(null, form({ principalPrn: PRINCIPAL_PRN, nodePrn: ORG_PRN }))],
  ['detachMembershipAction', () => detachMembershipAction(null, form({ id: '0190a1d4-0000-7000-8000-0000000000c1' }))],
  ['createTeamAction', () => createTeamAction(null, form({ orgPrn: ORG_PRN, slug: 'platform', name: 'Platform' }))],
  ['createProjectAction', () => createProjectAction(null, form({ teamPrn: TEAM_PRN, slug: 'api', name: 'API' }))],
] as const;

/** The nine lifecycle actions (SMA-630), each with a form its zod schema accepts. */
const LIFECYCLE_ACTIONS = [
  ['renameOrganizationAction', () => renameOrganizationAction(null, form(rename(ORG_PRN)))],
  ['archiveOrganizationAction', () => archiveOrganizationAction(null, form({ prn: ORG_PRN }))],
  ['restoreOrganizationAction', () => restoreOrganizationAction(null, form({ prn: ORG_PRN }))],
  ['renameTeamAction', () => renameTeamAction(null, form(rename(TEAM_PRN)))],
  ['archiveTeamAction', () => archiveTeamAction(null, form({ prn: TEAM_PRN }))],
  ['restoreTeamAction', () => restoreTeamAction(null, form({ prn: TEAM_PRN }))],
  ['renameProjectAction', () => renameProjectAction(null, form(rename(PROJECT_PRN)))],
  ['archiveProjectAction', () => archiveProjectAction(null, form({ prn: PROJECT_PRN }))],
  ['restoreProjectAction', () => restoreProjectAction(null, form({ prn: PROJECT_PRN }))],
] as const;

/** The same nine, each with a form its zod schema refuses (an empty PRN). */
const LIFECYCLE_INVALID = [
  ['renameOrganizationAction', () => renameOrganizationAction(null, form(rename('')))],
  ['archiveOrganizationAction', () => archiveOrganizationAction(null, form({ prn: '' }))],
  ['restoreOrganizationAction', () => restoreOrganizationAction(null, form({ prn: '' }))],
  ['renameTeamAction', () => renameTeamAction(null, form(rename('')))],
  ['archiveTeamAction', () => archiveTeamAction(null, form({ prn: '' }))],
  ['restoreTeamAction', () => restoreTeamAction(null, form({ prn: '' }))],
  ['renameProjectAction', () => renameProjectAction(null, form(rename('')))],
  ['archiveProjectAction', () => archiveProjectAction(null, form({ prn: '' }))],
  ['restoreProjectAction', () => restoreProjectAction(null, form({ prn: '' }))],
] as const;

const LAYOUT_REFRESH = [{ path: '/orgs', type: 'layout' }];

describe('every Server Action revalidates /orgs with the layout scope', () => {
  it.each([...ACTIONS, ...LIFECYCLE_ACTIONS])('%s', async (_name, run) => {
    const result = await run();

    expect(result).toEqual({ ok: true });
    // Strict equality on BOTH arguments: 'layout' is what refreshes the list the parent layout
    // renders, and 'page' would leave a created node invisible until a manual reload.
    expect(revalidatedPaths).toEqual(LAYOUT_REFRESH);
  });

  // The other direction: the call is inside `if (result.ok)`, so a refused action must revalidate
  // NOTHING. Without this, an action that revalidated unconditionally would still pass above.
  it.each(ACTIONS)('%s revalidates nothing when IAM refuses', async (_name, run) => {
    failNext(Code.PermissionDenied);

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

// SMA-630 spec § 4.4. For a lifecycle action a `forbidden` or `conflict` refusal often means that
// the page is stale, so these also refresh. `invalid-input` does not.
describe('the nine lifecycle actions also revalidate on forbidden and conflict', () => {
  it.each(LIFECYCLE_ACTIONS)('%s revalidates when IAM answers forbidden', async (_name, run) => {
    failNext(Code.PermissionDenied);

    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual(LAYOUT_REFRESH);
  });

  it.each(LIFECYCLE_ACTIONS)('%s revalidates when IAM answers conflict', async (_name, run) => {
    failNext(Code.AlreadyExists);

    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'conflict' } });
    expect(revalidatedPaths).toEqual(LAYOUT_REFRESH);
  });

  it.each(LIFECYCLE_ACTIONS)('%s revalidates nothing when IAM answers invalid-input', async (_name, run) => {
    failNext(Code.InvalidArgument);

    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it.each(LIFECYCLE_INVALID)('%s revalidates nothing when the form is invalid', async (_name, run) => {
    const result = await run();

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(revalidatedPaths).toEqual([]);
  });
});

// SMA-629 spec § 6.4. The two dead-letter actions refresh the ONE page, on ok and on not-found (the
// row is stale), and never on forbidden: the refreshed list read would call forbidden() and replace
// the inline error with the 403 view.
const DEAD_LETTER_ID = '0190a1f0-0000-7000-8000-0000000000d1';
const PAGE_REFRESH = [{ path: '/dead-letters', type: 'page' }];
const DEAD_LETTER_ACTIONS = [
  ['replayDeadLetterAction', (id: string) => replayDeadLetterAction(null, form({ id })), outbox.replayDeadLetter],
  ['discardDeadLetterAction', (id: string) => discardDeadLetterAction(null, form({ id })), outbox.discardDeadLetter],
] as const;

describe('the two dead-letter actions', () => {
  it.each(DEAD_LETTER_ACTIONS)('%s sends the trimmed id and refreshes the page on a success', async (_name, run, rpc) => {
    const result = await run(` ${DEAD_LETTER_ID} `);

    expect(result).toEqual({ ok: true });
    expect(rpc.mock.calls.at(-1)?.[0]).toEqual({ id: DEAD_LETTER_ID });
    expect(revalidatedPaths).toEqual(PAGE_REFRESH);
  });

  it.each(DEAD_LETTER_ACTIONS)('%s refreshes the page when IAM answers not-found', async (_name, run) => {
    failNext(Code.NotFound);

    const result = await run(DEAD_LETTER_ID);

    expect(result).toMatchObject({ ok: false, error: { presentation: 'not-found' } });
    expect(revalidatedPaths).toEqual(PAGE_REFRESH);
  });

  it.each(DEAD_LETTER_ACTIONS)('%s refreshes nothing when IAM answers forbidden', async (_name, run) => {
    failNext(Code.PermissionDenied);

    const result = await run(DEAD_LETTER_ID);

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual([]);
  });

  it.each(DEAD_LETTER_ACTIONS)('%s refreshes nothing when IAM is degraded', async (_name, run) => {
    failNext(Code.Unavailable);

    const result = await run(DEAD_LETTER_ID);

    expect(result).toMatchObject({ ok: false, error: { presentation: 'degraded' } });
    expect(revalidatedPaths).toEqual([]);
  });

  // § 7.3: this check belongs at the action level, because the command does not validate.
  it.each(DEAD_LETTER_ACTIONS)('%s refuses an id that is not a UUID and makes ZERO IAM calls', async (_name, run, rpc) => {
    const before = rpc.mock.calls.length;

    const result = await run('not-a-uuid');

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(rpc.mock.calls.length).toBe(before);
    expect(revalidatedPaths).toEqual([]);
  });
});

// SMA-661 spec § 6.5, § 6.6 and AC 1. The action parses the form, so a bulk replay with no valid
// max_rows never reaches IAM. On a success it refreshes the one page; on a failure it refreshes
// nothing (bulk replay never answers not-found).
const bulkForm = (fields: Record<string, string>): FormData => form({ eventType: '', parkedFrom: '', parkedTo: '', ...fields });

describe('the bulk-replay action', () => {
  it('sends the budget as a bigint and the canonical bound, answers the count, and refreshes the page', async () => {
    const result = await bulkReplayDeadLettersAction(null, bulkForm({ maxRows: '500', parkedFrom: '2026-09-19 00:00Z' }));

    expect(result).toEqual({ ok: true, replayed: 3 });
    expect(outbox.bulkReplayDeadLetters.mock.calls.at(-1)?.[0]).toMatchObject({ eventType: '', maxRows: 500n, parkedFrom: { seconds: 1_789_776_000n, nanos: 0 } });
    expect(revalidatedPaths).toEqual(PAGE_REFRESH);
  });

  it.each<[string, Record<string, string>]>([
    ['no maxRows field at all', {}],
    ['an empty maxRows', { maxRows: '' }],
    ['zero', { maxRows: '0' }],
    ['one past the ceiling', { maxRows: '10001' }],
    ['a hexadecimal budget', { maxRows: '0x10' }],
    ['a fractional budget', { maxRows: '7.5' }],
    ['a parked bound with no zone', { maxRows: '5', parkedFrom: '2026-09-19T00:00:00' }],
  ])('refuses %s as invalid input and makes ZERO IAM calls (AC 1)', async (_label, fields) => {
    const before = outbox.bulkReplayDeadLetters.mock.calls.length;

    const result = await bulkReplayDeadLettersAction(null, bulkForm(fields));

    expect(result).toMatchObject({ ok: false, error: { presentation: 'invalid-input' } });
    expect(outbox.bulkReplayDeadLetters.mock.calls.length).toBe(before);
    expect(revalidatedPaths).toEqual([]);
  });

  it('refreshes nothing when IAM answers forbidden', async () => {
    failNext(Code.PermissionDenied);

    const result = await bulkReplayDeadLettersAction(null, bulkForm({ maxRows: '5' }));

    expect(result).toMatchObject({ ok: false, error: { presentation: 'forbidden' } });
    expect(revalidatedPaths).toEqual([]);
  });
});
