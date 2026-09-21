// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The bulk-replay form (SMA-661 spec § 6.2–§ 6.4, AC 2). The pure texts first, then the REAL form
// inside the REAL frame: the confirmation cannot open without a valid budget, one form submits the
// budget and the three hidden scope fields, a step-1 submission (Enter) runs nothing, and every
// button waits while the frame is busy.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { BulkReplayForm, bulkReplayConfirmation, bulkReplayScopeText, maxRowsOf, type BulkReplayScope } from '../../app/(console)/dead-letters/bulk-replay-form';
import type { BulkReplayAction } from '../../app/(console)/dead-letters/commands';
import { DeadLetterTable } from '../../app/(console)/dead-letters/dead-letter-table';
import { DeadLettersFrame, type DeadLetterActions } from '../../app/(console)/dead-letters/dead-letters-frame';
import type { DeadLetterRow } from '../../app/(console)/dead-letters/load';
import { PAGE_SIZE } from '../../lib/paging';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/dead-letters' }));
// The frame's FormError chain must not load the whole server runtime into jsdom (see dead-letters-frame.test.tsx).
vi.mock('@paigasus/console-core', () => ({ requestPath: () => Promise.resolve('/iam/dead-letters') }));

afterEach(() => {
  cleanup();
});

const CEILING = 10_000;
const FROM = '2026-09-19T00:00:00.000Z';
const TO = '2026-09-20T00:00:00.000Z';
const FULL: BulkReplayScope = { eventType: 'iam.team.created', parkedFrom: FROM, parkedTo: TO };
const EMPTY: BulkReplayScope = { eventType: '', parkedFrom: '', parkedTo: '' };

const BEYOND = 'The scope can hold events this page does not show. A page holds 50 events, and the budget can be larger.';
const RISKS =
  'A replay can publish an event a second time when the broker deduplication window has passed. A cancelled or timed-out request can leave an unknown number of events already replayed; running it again is safe, because a replayed event is no longer parked.';
const NO_PARKED_TIME = 'An event with no parked time is not in this scope.';

describe('bulkReplayConfirmation (§ 6.4, AC 2)', () => {
  it('names the budget, the full scope with both bounds included, the page, the risks and the blind spot', () => {
    expect(bulkReplayConfirmation({ ...FULL, maxRows: 500 })).toBe(
      `Replay up to 500 parked events. Scope: event type iam.team.created, parked from ${FROM} to ${TO}, both bounds included. ${BEYOND} ${RISKS} ${NO_PARKED_TIME}`,
    );
  });

  it('names an absent bound as absent', () => {
    expect(bulkReplayConfirmation({ eventType: '', parkedFrom: FROM, parkedTo: '', maxRows: 5 })).toBe(
      `Replay up to 5 parked events. Scope: any event type, parked from ${FROM}, the bound included, with no end. ${BEYOND} ${RISKS} ${NO_PARKED_TIME}`,
    );
    expect(bulkReplayConfirmation({ eventType: '', parkedFrom: '', parkedTo: TO, maxRows: 5 })).toBe(
      `Replay up to 5 parked events. Scope: any event type, parked up to ${TO}, the bound included, with no start. ${BEYOND} ${RISKS} ${NO_PARKED_TIME}`,
    );
  });

  it('names the empty scope in words, with no blind-spot sentence (D5)', () => {
    expect(bulkReplayConfirmation({ ...EMPTY, maxRows: 10_000 })).toBe(
      `Replay up to 10000 parked events. The scope is EVERY parked event: no event type and no parked-time window. ${BEYOND} ${RISKS}`,
    );
  });

  it('adds no blind-spot sentence for an event type with no window', () => {
    expect(bulkReplayConfirmation({ eventType: 'iam.team.created', parkedFrom: '', parkedTo: '', maxRows: 2 })).toBe(
      `Replay up to 2 parked events. Scope: event type iam.team.created, any parked time. ${BEYOND} ${RISKS}`,
    );
  });

  it('uses the singular for a budget of one', () => {
    expect(bulkReplayConfirmation({ ...FULL, maxRows: 1 }).startsWith('Replay up to 1 parked event. ')).toBe(true);
  });

  it('states the page size that lib/paging.ts really uses', () => {
    expect(bulkReplayConfirmation({ ...EMPTY, maxRows: 1 })).toContain(`A page holds ${String(PAGE_SIZE)} events`);
  });
});

describe('bulkReplayScopeText (§ 6.3)', () => {
  it.each<[BulkReplayScope, string]>([
    [FULL, `Scope: event type iam.team.created, parked ${FROM} → ${TO}`],
    [{ eventType: 'iam.team.created', parkedFrom: FROM, parkedTo: '' }, `Scope: event type iam.team.created, parked ${FROM} → (no end)`],
    [{ eventType: '', parkedFrom: '', parkedTo: TO }, `Scope: any event type, parked (no start) → ${TO}`],
    [EMPTY, 'Scope: any event type, any parked time'],
  ])('%j reads %s', (scope, text) => {
    expect(bulkReplayScopeText(scope)).toBe(text);
  });
});

describe('maxRowsOf (§ 6.4)', () => {
  it('reads one to five digits from 1 to the ceiling', () => {
    expect(maxRowsOf('1', CEILING)).toBe(1);
    expect(maxRowsOf(' 500 ', CEILING)).toBe(500);
    expect(maxRowsOf('10000', CEILING)).toBe(10_000);
  });

  it.each(['', '0', '10001', '99999', '100000', '7.5', '0x10', '+7', '7.', '1e3', '-1'])('refuses %j', (raw) => {
    expect(maxRowsOf(raw, CEILING)).toBeNull();
  });
});

const ROW_A: DeadLetterRow = {
  id: '0190a1f0-0000-7000-8000-0000000000a1',
  eventType: 'iam.team.created',
  aggregatePrn: 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1',
  payload: '{}',
  schemaVersion: 1,
  attempts: 5,
  parkedAt: '2026-09-19T00:05:00.000Z',
  occurredAt: '2026-09-19T00:00:00.000Z',
  actorPrn: null,
  correlationId: null,
  lastError: null,
};

type Sig = (previous: ActionState, form: FormData) => Promise<ActionState>;

function actions(overrides: Partial<Record<'replay' | 'discard', Sig>> & { readonly bulkReplay?: BulkReplayAction } = {}) {
  return {
    replay: vi.fn<Sig>(overrides.replay ?? (() => Promise.resolve({ ok: true }))),
    discard: vi.fn<Sig>(overrides.discard ?? (() => Promise.resolve({ ok: true }))),
    bulkReplay: vi.fn<BulkReplayAction>(overrides.bulkReplay ?? (() => Promise.resolve({ ok: true, replayed: 2 }))),
  };
}

function page(a: DeadLetterActions, scope: BulkReplayScope = FULL): ReactNode {
  return (
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <DeadLettersFrame actions={a}>
        <BulkReplayForm scope={scope} ceiling={CEILING} />
        <DeadLetterTable rows={[ROW_A]} />
      </DeadLettersFrame>
    </ZoneProvider>
  );
}

const region = (): HTMLElement => screen.getByTestId('dead-letters-result');
const bulk = (): HTMLElement => screen.getByRole('form', { name: 'Bulk replay' });
const firstButton = (): HTMLButtonElement => within(bulk()).getByRole<HTMLButtonElement>('button', { name: 'Bulk replay…' });
const maxRowsInput = (): HTMLInputElement => within(bulk()).getByLabelText<HTMLInputElement>('Max rows');

describe('the form (§ 6.3, § 6.4)', () => {
  it('shows the scope as text and the range before anything is typed', () => {
    render(page(actions()));

    expect(screen.getByRole('heading', { name: 'Bulk replay' })).toBeDefined();
    expect(screen.getByText(bulkReplayScopeText(FULL))).toBeDefined();
    expect(screen.getByText('Enter a whole number from 1 to 10000.')).toBeDefined();
    expect(firstButton().disabled).toBe(true);
  });

  it.each(['0', '10001', '7.5', '0x10'])('keeps the first button disabled for %j, so no confirmation opens without a valid number', async (value) => {
    const user = userEvent.setup();
    render(page(actions()));

    await user.type(maxRowsInput(), value);

    expect(firstButton().disabled).toBe(true);
  });

  it('asks first; Cancel returns; Confirm submits the budget and the three scope fields from ONE form', async () => {
    const user = userEvent.setup();
    const a = actions();
    render(page(a));

    await user.type(maxRowsInput(), '500');
    await user.click(firstButton());
    const text = bulkReplayConfirmation({ ...FULL, maxRows: 500 });
    expect(within(bulk()).getByText(text)).toBeDefined();
    expect(maxRowsInput().readOnly).toBe(true);
    await user.click(within(bulk()).getByRole('button', { name: 'Cancel' }));
    expect(within(bulk()).queryByText(text)).toBeNull();
    expect(a.bulkReplay).not.toHaveBeenCalled();

    await user.click(firstButton());
    await user.click(within(bulk()).getByRole('button', { name: 'Confirm bulk replay' }));

    await within(region()).findByText('Replayed 2 events.');
    expect(a.bulkReplay).toHaveBeenCalledTimes(1);
    const form = a.bulkReplay.mock.calls[0]?.[1];
    expect(form?.get('maxRows')).toBe('500');
    expect(form?.get('eventType')).toBe(FULL.eventType);
    expect(form?.get('parkedFrom')).toBe(FROM);
    expect(form?.get('parkedTo')).toBe(TO);
    expect(within(bulk()).queryByRole('button', { name: 'Confirm bulk replay' })).toBeNull();
  });

  it('runs nothing when the form is submitted in step 1, as Enter in the input does', () => {
    const a = actions();
    render(page(a));

    fireEvent.change(maxRowsInput(), { target: { value: '500' } });
    fireEvent.submit(bulk());

    expect(a.bulkReplay).not.toHaveBeenCalled();
  });

  it('disables the buttons of both steps while the frame is busy', async () => {
    const user = userEvent.setup();
    let finish: (state: ActionState) => void = () => undefined;
    render(
      page(
        actions({
          replay: () =>
            new Promise<ActionState>((resolve) => {
              finish = resolve;
            }),
        }),
      ),
    );

    await user.type(maxRowsInput(), '500');
    await user.click(firstButton());
    const confirm = within(bulk()).getByRole<HTMLButtonElement>('button', { name: 'Confirm bulk replay' });
    const cancel = within(bulk()).getByRole<HTMLButtonElement>('button', { name: 'Cancel' });

    // A row replay makes the frame busy. isPending can lag the transition's setState, so wait for it.
    await user.click(screen.getByRole('button', { name: 'Replay' }));
    await waitFor(() => {
      expect(confirm.disabled).toBe(true);
      expect(cancel.disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(cancel.disabled).toBe(false);
    });

    // Step 1 too: with a valid budget, the first button still waits while the frame is busy.
    await user.click(cancel);
    await user.click(screen.getByRole('button', { name: 'Replay' }));
    await waitFor(() => {
      expect(firstButton().disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(firstButton().disabled).toBe(false);
    });
  });
});
