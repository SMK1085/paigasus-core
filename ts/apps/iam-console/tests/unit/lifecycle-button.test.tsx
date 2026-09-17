// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The two lifecycle controls (SMA-630 spec § 6.2). ArchiveButton asks in two steps with local
// state, and has NO submit button before the first click. RestoreButton submits at once.
import type { ReactElement, ReactNode } from 'react';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
import { ArchiveButton, archiveConfirmation, RestoreButton } from '../../app/_components/lifecycle-button';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

afterEach(() => {
  cleanup();
});

const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000e1:team/0190a1b2-0000-7000-8000-0000000000e2';
const CONFIRMATION = 'Archive Platform Team? Until you restore it, IAM refuses changes to it and to everything under it, and the AI Gateway refuses model calls for everything under it.';

const FORBIDDEN: PaigasusError = {
  presentation: 'forbidden',
  domain: ErrorDomain.IAM,
  reason: ErrorReason.FORBIDDEN,
  rawReason: 'forbidden',
  rawDomain: 'iam.paigasus.io',
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-archive',
  requestId: null,
  retryable: false,
  metadata: {},
  transport: { kind: 'http', status: 403 },
};

// An explicit type argument gives the mock its 2-argument arity (so `.mock.calls[0]?.[1]` below
// type-checks) without a named-but-unused parameter, which would trip @typescript-eslint/no-unused-vars
// — this repo's eslint config carries no `argsIgnorePattern: '^_'` (paigasus-sdk/tests/transport-wiring.test.ts).
function actionAnswering(state: ActionState) {
  return vi.fn<(previous: ActionState, form: FormData) => Promise<ActionState>>(() => Promise.resolve(state));
}

function renderInZone(element: ReactElement) {
  return render(element, {
    wrapper: ({ children }: { children: ReactNode }) => (
      <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
        {children}
      </ZoneProvider>
    ),
  });
}

describe('ArchiveButton', () => {
  it('shows no form and no submit button before the first click', () => {
    const { container } = renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={actionAnswering({ ok: true })} />);

    expect(screen.getByRole('button', { name: 'Archive' }).getAttribute('type')).toBe('button');
    expect(container.querySelector('form')).toBeNull();
    expect(container.querySelector('button[type="submit"]')).toBeNull();
  });

  it('asks with the exact spec text, and Cancel returns to the first state', async () => {
    const user = userEvent.setup();
    const { container } = renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={actionAnswering({ ok: true })} />);

    await user.click(screen.getByRole('button', { name: 'Archive' }));

    expect(archiveConfirmation('Platform Team')).toBe(CONFIRMATION);
    expect(screen.getByText(CONFIRMATION).textContent).toBe(CONFIRMATION);
    expect(screen.getByRole('button', { name: 'Confirm archive' }).getAttribute('type')).toBe('submit');
    expect(screen.getByRole('button', { name: 'Cancel' }).getAttribute('type')).toBe('button');
    expect(screen.queryByRole('button', { name: 'Archive' })).toBeNull();

    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(screen.getByRole('button', { name: 'Archive' }).getAttribute('type')).toBe('button');
    expect(screen.queryByText(CONFIRMATION)).toBeNull();
    expect(container.querySelector('button[type="submit"]')).toBeNull();
  });

  // The Manage section's result region shows "Archived." and "Restored." (spec § 6.4,
  // manage-controls.test.tsx). The controls show no success text, so the text never shows twice.
  it('posts the node PRN on confirm and shows no success text of its own', async () => {
    const user = userEvent.setup();
    const action = actionAnswering({ ok: true });
    renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={action} />);

    await user.click(screen.getByRole('button', { name: 'Archive' }));
    const confirm = screen.getByRole<HTMLButtonElement>('button', { name: 'Confirm archive' });
    await user.click(confirm);

    await waitFor(() => {
      expect(action).toHaveBeenCalledTimes(1);
      expect(confirm.disabled).toBe(false);
    });
    expect(screen.queryByText('Archived.')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
    expect(action.mock.calls[0]?.[1].get('prn')).toBe(TEAM_PRN);
  });

  it('shows a refusal with its copy and the correlation id in its error area', async () => {
    const user = userEvent.setup();
    renderInZone(<ArchiveButton testId="archive-team" prn={TEAM_PRN} name="Platform Team" action={actionAnswering({ ok: false, error: FORBIDDEN })} />);

    await user.click(screen.getByRole('button', { name: 'Archive' }));
    await user.click(screen.getByRole('button', { name: 'Confirm archive' }));

    const area = screen.getByTestId('archive-team-error');
    expect(await within(area).findByText(FORM_REASON_COPY[ErrorReason.FORBIDDEN] ?? '')).toBeDefined();
    expect(within(area).getByTestId('correlation-id').textContent).toBe('corr-unit-archive');
  });
});

describe('RestoreButton', () => {
  it('has a submit button at once, posts the node PRN and shows no success text of its own', async () => {
    const user = userEvent.setup();
    const action = actionAnswering({ ok: true });
    renderInZone(<RestoreButton testId="restore-team" prn={TEAM_PRN} action={action} />);

    const button = screen.getByRole<HTMLButtonElement>('button', { name: 'Restore' });
    expect(button.getAttribute('type')).toBe('submit');
    expect(screen.getByTestId('restore-team').tagName).toBe('FORM');
    expect(screen.getByTestId('restore-team-error')).toBeDefined();

    await user.click(button);

    await waitFor(() => {
      expect(action).toHaveBeenCalledTimes(1);
      expect(button.disabled).toBe(false);
    });
    expect(screen.queryByText('Restored.')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
    expect(action.mock.calls[0]?.[1].get('prn')).toBe(TEAM_PRN);
  });
});
