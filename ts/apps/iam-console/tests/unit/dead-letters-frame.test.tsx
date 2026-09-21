// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The dead-letters frame (SMA-629 spec § 6.4, § 7.2). Each case renders the REAL frame and table with
// real transitions and real form submissions, then renders it AGAIN with the children that the
// revalidated page sends — the technique of manage-controls.test.tsx and the gateway console's
// service-account-section.test.tsx.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import type { PaigasusError, Presentation } from '@paigasus/sdk/errors/types';
import { DEAD_LETTER_GONE, PRESENTATION_COPY } from '../../app/_components/error-copy';
import { SectionError } from '../../app/_components/section-error';
import { DeadLetterTable } from '../../app/(console)/dead-letters/dead-letter-table';
import { bulkReplayedText, DeadLettersFrame, discardConfirmation, UNREACHED_TEXT, useRunner, type DeadLetterActions } from '../../app/(console)/dead-letters/dead-letters-frame';
import type { BulkReplayAction, BulkReplayState } from '../../app/(console)/dead-letters/commands';
import type { DeadLetterRow } from '../../app/(console)/dead-letters/load';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/dead-letters' }));
// SectionError's errorTail imports requestPath from the package root, which would load the whole
// server runtime into jsdom. Only a relogin error reads it, and no case here uses one.
vi.mock('@paigasus/console-core', () => ({ requestPath: () => Promise.resolve('/iam/dead-letters') }));

afterEach(() => {
  cleanup();
});

const A = '0190a1f0-0000-7000-8000-0000000000a1';
const B = '0190a1f0-0000-7000-8000-0000000000b1';

function row(id: string, overrides: Partial<DeadLetterRow> = {}): DeadLetterRow {
  return {
    id,
    eventType: 'iam.team.created',
    aggregatePrn: 'prn:pgs:iam::0190a100-0000-7000-8000-00000000000a:team/0190a1b2-0000-7000-8000-0000000000a1',
    payload: '{"slug":"platform"}',
    schemaVersion: 1,
    attempts: 5,
    parkedAt: '2026-09-01T10:05:00.000Z',
    occurredAt: '2026-09-01T10:00:00.000Z',
    actorPrn: 'prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0',
    correlationId: 'corr-dead-letter-a',
    lastError: 'nats: timeout',
    ...overrides,
  };
}

const ROW_A = row(A);
const ROW_B = row(B, { actorPrn: null, correlationId: null, lastError: null });

function errorWith(presentation: Presentation): PaigasusError {
  return {
    presentation,
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'corr-unit-dead-letters',
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 5, codeName: 'NotFound' },
  };
}

type Sig = (previous: ActionState, form: FormData) => Promise<ActionState>;

function actions(overrides: Partial<Record<'replay' | 'discard', Sig>> & { readonly bulkReplay?: BulkReplayAction } = {}) {
  return {
    replay: vi.fn<Sig>(overrides.replay ?? (() => Promise.resolve({ ok: true }))),
    discard: vi.fn<Sig>(overrides.discard ?? (() => Promise.resolve({ ok: true }))),
    bulkReplay: vi.fn<BulkReplayAction>(overrides.bulkReplay ?? (() => Promise.resolve({ ok: true, replayed: 2 }))),
  };
}

/** A stand-in for the bulk form: it hands the runner a FormData with NO id, as the real form does. */
function BulkProbe(): ReactElement {
  const runner = useRunner();
  return (
    <button
      type="button"
      onClick={() => {
        const form = new FormData();
        form.append('maxRows', '5');
        runner.runBulk(form);
      }}
    >
      Run bulk
    </button>
  );
}

function frame(children: ReactNode, a: DeadLetterActions): ReactNode {
  return (
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <DeadLettersFrame actions={a}>{children}</DeadLettersFrame>
    </ZoneProvider>
  );
}

/** Renders the children of the revalidated page, with an awaited act() (see manage-controls.test.tsx). */
async function refresh(rerender: (ui: ReactNode) => void, ui: ReactNode): Promise<void> {
  await act(() => {
    rerender(ui);
    return Promise.resolve();
  });
}

const region = (): HTMLElement => screen.getByTestId('dead-letters-result');
const controlsOf = (id: string): HTMLElement => screen.getByTestId(`dead-letter-controls-${id}`);
const rowButtons = (): HTMLButtonElement[] => screen.getAllByRole<HTMLButtonElement>('button', { name: /^(Replay|Discard)$/ });

describe('one result region: a result survives every refresh (§ 6.4)', () => {
  it('keeps "Replayed event A." after the row goes, the list empties, and the table becomes an error', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, a));

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Replay' }));
    await within(region()).findByText(`Replayed event ${A}.`);
    expect(a.replay).toHaveBeenCalledTimes(1);
    expect(a.replay.mock.calls[0]?.[0]).toBeNull();
    expect(a.replay.mock.calls[0]?.[1].get('id')).toBe(A);

    await refresh(rerender, frame(<DeadLetterTable rows={[ROW_B]} />, a));
    expect(screen.queryByTestId(`dead-letter-controls-${A}`)).toBeNull();
    expect(within(region()).getByText(`Replayed event ${A}.`)).toBeDefined();

    await refresh(rerender, frame(<p>No dead letters</p>, a));
    expect(within(region()).getByText(`Replayed event ${A}.`)).toBeDefined();

    const sectionError = await SectionError({ error: errorWith('degraded') });
    await refresh(rerender, frame(sectionError, a));
    expect(screen.getByTestId('section-error')).toBeDefined();
    expect(within(region()).getByText(`Replayed event ${A}.`)).toBeDefined();
  });
});

describe('the runner', () => {
  it('shows a client-built error for a rejected action and does not throw (the frame stays mounted)', async () => {
    const user = userEvent.setup();
    render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, actions({ replay: () => Promise.reject(new TypeError('Failed to fetch')) })));

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Replay' }));

    expect(await within(region()).findByText(UNREACHED_TEXT)).toBeDefined();
    expect(within(region()).getByTestId('form-error').getAttribute('data-presentation')).toBe('generic');
    expect(controlsOf(B)).toBeDefined();
  });

  it('shows the NEWER submission’s result when an older one completes later', async () => {
    let finishFirst: (state: ActionState) => void = () => undefined;
    const a = actions({
      replay: (_previous, form) =>
        form.get('id') === A
          ? new Promise<ActionState>((resolve) => {
              finishFirst = resolve;
            })
          : Promise.resolve({ ok: false, error: errorWith('forbidden') }),
    });
    render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, a));

    // fireEvent.submit reaches onSubmit although the buttons are disabled while A runs.
    fireEvent.submit(screen.getByRole('form', { name: `Replay event ${A}` }));
    fireEvent.submit(screen.getByRole('form', { name: `Replay event ${B}` }));
    await within(region()).findByText(PRESENTATION_COPY.forbidden.body);
    await act(async () => {
      finishFirst({ ok: true });
      await Promise.resolve();
    });

    expect(within(region()).queryByText(`Replayed event ${A}.`)).toBeNull();
    expect(within(region()).getByTestId('form-error').getAttribute('data-presentation')).toBe('forbidden');
    // Only not-found gets the dead-letter sentence (§ 6.5).
    expect(within(region()).queryByText(DEAD_LETTER_GONE)).toBeNull();
  });

  it('disables every Replay and Discard button while an action runs, and a confirming row’s Confirm discard and Cancel too', async () => {
    const user = userEvent.setup();
    let finish: (state: ActionState) => void = () => undefined;
    render(
      frame(
        <DeadLetterTable rows={[ROW_A, ROW_B]} />,
        actions({
          replay: () =>
            new Promise<ActionState>((resolve) => {
              finish = resolve;
            }),
        }),
      ),
    );

    // Row B opens its discard confirmation BEFORE row A's replay starts, so its Confirm discard
    // and Cancel buttons already exist when the frame goes busy.
    await user.click(within(controlsOf(B)).getByRole('button', { name: 'Discard' }));
    const confirmDiscard = within(controlsOf(B)).getByRole<HTMLButtonElement>('button', { name: 'Confirm discard' });
    const cancel = within(controlsOf(B)).getByRole<HTMLButtonElement>('button', { name: 'Cancel' });

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Replay' }));

    await waitFor(() => {
      // Row B is confirming, so its own "Discard" button is replaced by Confirm discard and
      // Cancel: row A's Replay and Discard plus row B's Replay is 3, not 4.
      expect(rowButtons()).toHaveLength(3);
      for (const button of rowButtons()) expect(button.disabled).toBe(true);
      expect(confirmDiscard.disabled).toBe(true);
      expect(cancel.disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      for (const button of rowButtons()) expect(button.disabled).toBe(false);
      expect(confirmDiscard.disabled).toBe(false);
      expect(cancel.disabled).toBe(false);
    });
  });
});

describe('discard', () => {
  it('asks first with the exact text; Cancel returns, and Confirm discard submits the id', async () => {
    const user = userEvent.setup();
    const a = actions();
    render(frame(<DeadLetterTable rows={[ROW_A, ROW_B]} />, a));

    expect(discardConfirmation(A)).toBe(`Discard event ${A}? IAM deletes it from the dead-letter queue, and it is not published again. The audit log keeps a copy of the event.`);
    expect(within(controlsOf(A)).getByRole('button', { name: 'Discard' }).getAttribute('type')).toBe('button');

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Discard' }));
    expect(within(controlsOf(A)).getByText(discardConfirmation(A))).toBeDefined();
    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Cancel' }));
    expect(within(controlsOf(A)).queryByText(discardConfirmation(A))).toBeNull();
    expect(a.discard).not.toHaveBeenCalled();

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Discard' }));
    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Confirm discard' }));

    await within(region()).findByText(`Discarded event ${A}.`);
    expect(a.discard).toHaveBeenCalledTimes(1);
    expect(a.discard.mock.calls[0]?.[1].get('id')).toBe(A);
  });

  it('shows DEAD_LETTER_GONE and the correlation id for a not-found answer', async () => {
    const user = userEvent.setup();
    render(frame(<DeadLetterTable rows={[ROW_A]} />, actions({ discard: () => Promise.resolve({ ok: false, error: errorWith('not-found') }) })));

    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Discard' }));
    await user.click(within(controlsOf(A)).getByRole('button', { name: 'Confirm discard' }));

    expect(await within(region()).findByText(DEAD_LETTER_GONE)).toBeDefined();
    expect(within(region()).getByTestId('correlation-id').textContent).toBe('corr-unit-dead-letters');
  });
});

describe('the table', () => {
  it('renders a hostile payload and last error as text, never as HTML', () => {
    const hostile = '</pre><script>alert(1)</script>';
    const { container } = render(frame(<DeadLetterTable rows={[row(A, { payload: hostile, lastError: hostile })]} />, actions()));

    expect(container.querySelector('script')).toBeNull();
    expect(screen.getByTestId('dead-letter-payload').textContent).toBe(hostile);
    expect(screen.getByTestId('dead-letter-last-error').textContent).toBe(hostile);
  });

  it('shows "—" for IAM’s "none" and keeps IAM’s order', () => {
    render(frame(<DeadLetterTable rows={[ROW_B, ROW_A]} />, actions()));

    const rows = screen.getAllByTestId('dead-letter-row');
    expect(rows.map((element) => element.getAttribute('data-id'))).toEqual([B, A]);
    expect(within(rows[0] as HTMLElement).getAllByText('—').length).toBeGreaterThan(0);
    expect(screen.getByText('Newest events first. With a parked-time filter set, an event with no parked time is not listed.')).toBeDefined();
  });

  it('hides Replay and keeps Discard when canReplay is false (SMA-661 D4)', () => {
    render(frame(<DeadLetterTable canReplay={false} rows={[ROW_A]} />, actions()));

    expect(within(controlsOf(A)).queryByRole('button', { name: 'Replay' })).toBeNull();
    expect(screen.queryByRole('form', { name: `Replay event ${A}` })).toBeNull();
    expect(within(controlsOf(A)).getByRole('button', { name: 'Discard' })).toBeDefined();
  });
});

describe('bulk replay through the runner (SMA-661 spec § 6.1, § 6.7)', () => {
  it('words the count: plural, singular, and zero as a real answer', () => {
    expect(bulkReplayedText(8)).toBe('Replayed 8 events.');
    expect(bulkReplayedText(1)).toBe('Replayed 1 event.');
    expect(bulkReplayedText(0)).toBe('Replayed 0 events. No parked event matched the scope.');
  });

  it('shows the count, reads no id, and keeps the result across a refresh', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(
      frame(
        <>
          <BulkProbe />
          <DeadLetterTable rows={[ROW_A]} />
        </>,
        a,
      ),
    );

    await user.click(screen.getByRole('button', { name: 'Run bulk' }));
    await within(region()).findByText('Replayed 2 events.');
    expect(a.bulkReplay).toHaveBeenCalledTimes(1);
    expect(a.bulkReplay.mock.calls[0]?.[0]).toBeNull();
    expect(a.bulkReplay.mock.calls[0]?.[1].get('id')).toBeNull();
    expect(a.bulkReplay.mock.calls[0]?.[1].get('maxRows')).toBe('5');
    expect(a.replay).not.toHaveBeenCalled();

    await refresh(
      rerender,
      frame(
        <>
          <BulkProbe />
          <p>No dead letters</p>
        </>,
        a,
      ),
    );
    expect(within(region()).getByText('Replayed 2 events.')).toBeDefined();
  });

  it('shows a failure as FormError, and never the dead-letter sentence', async () => {
    const user = userEvent.setup();
    render(frame(<BulkProbe />, actions({ bulkReplay: () => Promise.resolve({ ok: false, error: errorWith('not-found') }) })));

    await user.click(screen.getByRole('button', { name: 'Run bulk' }));

    expect((await within(region()).findByTestId('form-error')).getAttribute('data-presentation')).toBe('not-found');
    expect(within(region()).queryByText(DEAD_LETTER_GONE)).toBeNull();
  });

  it('shows the unreached text for a rejected bulk action', async () => {
    const user = userEvent.setup();
    render(frame(<BulkProbe />, actions({ bulkReplay: () => Promise.reject(new TypeError('Failed to fetch')) })));

    await user.click(screen.getByRole('button', { name: 'Run bulk' }));

    expect(await within(region()).findByText(UNREACHED_TEXT)).toBeDefined();
  });

  it('lets a NEWER row submission win over an older bulk submission', async () => {
    let finishBulk: (state: BulkReplayState) => void = () => undefined;
    const a = actions({
      bulkReplay: () =>
        new Promise<BulkReplayState>((resolve) => {
          finishBulk = resolve;
        }),
      replay: () => Promise.resolve({ ok: false, error: errorWith('forbidden') }),
    });
    render(
      frame(
        <>
          <BulkProbe />
          <DeadLetterTable rows={[ROW_A]} />
        </>,
        a,
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Run bulk' }));
    fireEvent.submit(screen.getByRole('form', { name: `Replay event ${A}` }));
    await within(region()).findByText(PRESENTATION_COPY.forbidden.body);
    await act(async () => {
      finishBulk({ ok: true, replayed: 9 });
      await Promise.resolve();
    });

    expect(within(region()).queryByText('Replayed 9 events.')).toBeNull();
    expect(within(region()).getByTestId('form-error').getAttribute('data-presentation')).toBe('forbidden');
  });

  it('disables every row button while a bulk replay runs', async () => {
    let finish: (state: BulkReplayState) => void = () => undefined;
    render(
      frame(
        <>
          <BulkProbe />
          <DeadLetterTable rows={[ROW_A, ROW_B]} />
        </>,
        actions({
          bulkReplay: () =>
            new Promise<BulkReplayState>((resolve) => {
              finish = resolve;
            }),
        }),
      ),
    );

    fireEvent.click(screen.getByRole('button', { name: 'Run bulk' }));

    // isPending can lag the transition's own setState, so wait for the busy state.
    await waitFor(() => {
      expect(rowButtons()).toHaveLength(4);
      for (const button of rowButtons()) expect(button.disabled).toBe(true);
    });
    await act(async () => {
      finish({ ok: true, replayed: 1 });
      await Promise.resolve();
    });
    await waitFor(() => {
      for (const button of rowButtons()) expect(button.disabled).toBe(false);
    });
  });
});
