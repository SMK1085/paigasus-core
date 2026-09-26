// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The "Model access for people" block in each state (SMA-676 spec § 5.1, § 7.2): disabled, denied,
// error, holders and candidates, empty, read-only, each truncation line, the not-a-member mark and its
// hiding, and the two forms carrying the right fields to their actions.
//
// Two controller rulings get their own tests here (F10 wording, controller review of task 11):
// - Copy only: a form that receives a failed result must show only the presentation's copy, never
//   IAM's message text, even when that text carries a marker unique to the test.
// - Remount: a `revalidatePath` follows every grant and revoke result, an error included. The error
//   text must stay visible after that revalidated render, because the form keeping it is not
//   remounted when its key (the grant id, or the candidate set) is unchanged.
import { act, cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement } from 'react';
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { peopleModelAccessBlock } from '../../app/(console)/people-model-access/block';
import { PEOPLE_COPY, type PeopleModelAccessOk, type PeopleModelAccessView } from '../../app/(console)/people-model-access/view';

// An explicit type argument gives each mock its 2-argument arity (so `.mock.calls[0]?.[1]` below
// type-checks) without a named-but-unused parameter, which would trip @typescript-eslint/no-unused-vars
// — this repo's eslint config carries no `argsIgnorePattern: '^_'` (paigasus-sdk/tests/transport-wiring.test.ts).
const actions = vi.hoisted(() => ({
  grant: vi.fn<(previous: unknown, form: FormData) => Promise<ActionState>>(() => Promise.resolve({ ok: true as const })),
  revoke: vi.fn<(previous: unknown, form: FormData) => Promise<ActionState>>(() => Promise.resolve({ ok: true as const })),
}));
vi.mock('../../app/(console)/people-model-access/actions', () => ({ grantModelAccessAction: actions.grant, revokeModelAccessAction: actions.revoke }));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/0190a100-0000-7000-8000-00000000000a' }));

// cmdk's CommandList measures itself with ResizeObserver, which jsdom lacks (the same stub
// ts/packages/paigasus-ui/tests/setup.ts installs for its combobox tests).
beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => {
  cleanup();
  actions.grant.mockClear();
  actions.revoke.mockClear();
});

const ORG = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000a';
const HOLDER = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000d1';
const OUTSIDER = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000d2';
const CANDIDATE = 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000d3';
const GRANT_ID = '0190a1d4-0000-7000-8000-0000000000d4';
const ERROR: PaigasusError = {
  presentation: 'degraded',
  domain: null,
  reason: null,
  rawReason: null,
  rawDomain: null,
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-people',
  requestId: null,
  retryable: true,
  metadata: {},
  transport: { kind: 'grpc', code: 14, codeName: 'Unavailable' },
};

function ok(patch: Partial<PeopleModelAccessOk> = {}): PeopleModelAccessOk {
  return {
    kind: 'ok',
    orgPrn: ORG,
    readOnly: false,
    holders: [
      { grantId: GRANT_ID, principalPrn: HOLDER, member: true },
      { grantId: '0190a1d4-0000-7000-8000-0000000000d5', principalPrn: OUTSIDER, member: false },
    ],
    candidates: [{ principalPrn: CANDIDATE }],
    flags: { canGrant: true, canRevoke: true, grantsTruncated: false, membersTruncated: false },
    ...patch,
  };
}

async function block(view: PeopleModelAccessView): Promise<ReactElement> {
  return (
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      {await peopleModelAccessBlock({ view })}
    </ZoneProvider>
  );
}

describe('peopleModelAccessBlock', () => {
  it('shows the disabled line (D15)', async () => {
    render(await block({ kind: 'disabled' }));
    expect(screen.getByTestId('people-model-access-disabled').textContent).toBe(PEOPLE_COPY.disabled);
    expect(screen.getByRole('heading', { name: PEOPLE_COPY.title })).toBeDefined();
  });

  it('shows the denied line', async () => {
    render(await block({ kind: 'denied' }));
    expect(screen.getByTestId('people-model-access-denied').textContent).toBe(PEOPLE_COPY.denied);
  });

  it('shows the section error, never IAM’s message', async () => {
    render(await block({ kind: 'error', error: ERROR }));
    expect(screen.getByTestId('section-error')).toBeDefined();
    expect(screen.queryByText('IAM text that must never show')).toBeNull();
  });

  it('lists holders with the not-a-member mark, and a Revoke per row', async () => {
    render(await block(ok()));
    const rows = screen.getAllByTestId('people-holder-row');
    expect(rows.map((row) => row.getAttribute('data-principal'))).toEqual([HOLDER, OUTSIDER]);
    expect(within(rows[0] as HTMLElement).queryByTestId('people-not-member')).toBeNull();
    expect(within(rows[1] as HTMLElement).getByTestId('people-not-member').textContent).toBe(PEOPLE_COPY.notMember);
    expect(within(rows[0] as HTMLElement).getByRole('button', { name: PEOPLE_COPY.revoke })).toBeDefined();
  });

  it('hides the mark when the member list is truncated (member: null), and shows both truncation lines', async () => {
    render(await block(ok({ holders: [{ grantId: GRANT_ID, principalPrn: HOLDER, member: null }], flags: { canGrant: true, canRevoke: true, grantsTruncated: true, membersTruncated: true } })));
    expect(screen.queryByTestId('people-not-member')).toBeNull();
    expect(screen.getByTestId('people-grants-truncated').textContent).toBe(PEOPLE_COPY.grantsTruncated);
    expect(screen.getByTestId('people-members-truncated').textContent).toBe(PEOPLE_COPY.membersTruncated);
  });

  it('shows the empty states', async () => {
    render(await block(ok({ holders: [], candidates: [] })));
    expect(screen.getByText(PEOPLE_COPY.noHolders)).toBeDefined();
    expect(screen.getByText(PEOPLE_COPY.noCandidates)).toBeDefined();
    expect(screen.queryByRole('combobox')).toBeNull();
  });

  it('is read-only: a note, and no control', async () => {
    render(await block(ok({ readOnly: true, flags: { canGrant: false, canRevoke: false, grantsTruncated: false, membersTruncated: false } })));
    expect(screen.getByTestId('people-read-only').textContent).toBe(PEOPLE_COPY.readOnly);
    expect(screen.queryByRole('button', { name: PEOPLE_COPY.revoke })).toBeNull();
    expect(screen.queryByRole('button', { name: PEOPLE_COPY.grant })).toBeNull();
  });

  it('grants the chosen candidate at the org', async () => {
    const user = userEvent.setup();
    render(await block(ok()));
    await user.click(screen.getByRole('combobox'));
    await user.click(await screen.findByRole('option', { name: CANDIDATE }));
    await user.click(screen.getByRole('button', { name: PEOPLE_COPY.grant }));
    expect(actions.grant).toHaveBeenCalledTimes(1);
    const form = actions.grant.mock.calls[0]?.[1];
    expect(form?.get('principalPrn')).toBe(CANDIDATE);
    expect(form?.get('orgPrn')).toBe(ORG);
  });

  it('revokes with the principal, the org and the grant id of the row', async () => {
    const user = userEvent.setup();
    render(await block(ok()));
    await user.click(within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).getByRole('button', { name: PEOPLE_COPY.revoke }));
    const form = actions.revoke.mock.calls[0]?.[1];
    expect([form?.get('principalPrn'), form?.get('orgPrn'), form?.get('grantId')]).toEqual([HOLDER, ORG, GRANT_ID]);
  });

  it('shows only the presentation copy for a failed grant, never IAM’s message (copy-only ruling)', async () => {
    const user = userEvent.setup();
    actions.grant.mockResolvedValueOnce({ ok: false, error: { ...ERROR, message: 'MARKER-GRANT-DO-NOT-SHOW' } });
    render(await block(ok()));
    await user.click(screen.getByRole('combobox'));
    await user.click(await screen.findByRole('option', { name: CANDIDATE }));
    await user.click(screen.getByRole('button', { name: PEOPLE_COPY.grant }));
    expect(await screen.findByTestId('form-error')).toBeDefined();
    expect(screen.queryByText('MARKER-GRANT-DO-NOT-SHOW')).toBeNull();
  });

  it('shows only the presentation copy for a failed revoke, never IAM’s message (copy-only ruling)', async () => {
    const user = userEvent.setup();
    actions.revoke.mockResolvedValueOnce({ ok: false, error: { ...ERROR, message: 'MARKER-REVOKE-DO-NOT-SHOW' } });
    render(await block(ok()));
    await user.click(within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).getByRole('button', { name: PEOPLE_COPY.revoke }));
    expect(await within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).findByTestId('form-error')).toBeDefined();
    expect(screen.queryByText('MARKER-REVOKE-DO-NOT-SHOW')).toBeNull();
  });

  it('keeps the grant form error across a revalidated render with the same candidates (remount ruling)', async () => {
    const user = userEvent.setup();
    actions.grant.mockResolvedValueOnce({ ok: false, error: ERROR });
    const { rerender } = render(await block(ok()));
    await user.click(screen.getByRole('combobox'));
    await user.click(await screen.findByRole('option', { name: CANDIDATE }));
    await user.click(screen.getByRole('button', { name: PEOPLE_COPY.grant }));
    expect(await screen.findByTestId('form-error')).toBeDefined();

    // The action's revalidatePath re-renders the server tree. The candidates are unchanged (the
    // grant failed), so GrantForm's key is unchanged, and its useActionState result survives.
    await act(async () => {
      rerender(await block(ok()));
    });

    expect(screen.getByTestId('form-error')).toBeDefined();
  });

  it('keeps the revoke form error across a revalidated render with the same holder row (remount ruling)', async () => {
    const user = userEvent.setup();
    actions.revoke.mockResolvedValueOnce({ ok: false, error: ERROR });
    const { rerender } = render(await block(ok()));
    await user.click(within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).getByRole('button', { name: PEOPLE_COPY.revoke }));
    expect(await within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).findByTestId('form-error')).toBeDefined();

    // The revoke failed, so the grant is still there: the row's key (its grantId) is unchanged,
    // and its useActionState result survives the revalidated render.
    await act(async () => {
      rerender(await block(ok()));
    });

    expect(within(screen.getAllByTestId('people-holder-row')[0] as HTMLElement).getByTestId('form-error')).toBeDefined();
  });
});
