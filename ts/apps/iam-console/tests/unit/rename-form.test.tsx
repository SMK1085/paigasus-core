// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The rename form (SMA-630 spec § 6.1). React 19 resets a form before it runs EVERY action, whatever
// the result. With uncontrolled inputs a failed rename would put the old values back, and "This
// slug is already in use" would show next to the old slug. The inputs are therefore controlled.
// The control case below proves that this harness really resets a form, so the main case is not
// green for the wrong reason.
import { useActionState, type ReactElement, type ReactNode } from 'react';
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import type { FormAction } from '../../app/_components/form-action';
import { RenameForm } from '../../app/_components/rename-form';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

afterEach(() => {
  cleanup();
});

const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000e1:team/0190a1b2-0000-7000-8000-0000000000e2';

const SLUG_CONFLICT: PaigasusError = {
  presentation: 'conflict',
  domain: ErrorDomain.IAM,
  reason: ErrorReason.SLUG_CONFLICT,
  rawReason: 'slug-conflict',
  rawDomain: 'iam.paigasus.io',
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-rename',
  requestId: null,
  retryable: false,
  metadata: {},
  transport: { kind: 'http', status: 409 },
};

// An explicit type argument gives the mock its 2-argument arity (so `.mock.calls[0]?.[1]` below
// type-checks) without a named-but-unused parameter, which would trip @typescript-eslint/no-unused-vars
// — this repo's eslint config carries no `argsIgnorePattern: '^_'` (paigasus-sdk/tests/transport-wiring.test.ts).
function failing() {
  return vi.fn<(previous: ActionState, form: FormData) => Promise<ActionState>>(() => Promise.resolve({ ok: false, error: SLUG_CONFLICT }));
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

/** An UNCONTROLLED form with the same action wiring: the control for the reset. */
function UncontrolledControl({ action }: { readonly action: FormAction }): ReactElement {
  const [state, formAction] = useActionState(action, null);
  return (
    <form action={formAction}>
      <input aria-label="Plain slug" name="slug" defaultValue="platform" />
      <button type="submit">Send</button>
      {state?.ok === false ? <p>failed</p> : null}
    </form>
  );
}

describe('RenameForm', () => {
  it('starts from the current values, and posts them as hidden fields with the typed values', async () => {
    const user = userEvent.setup();
    const action = failing();
    renderInZone(<RenameForm testId="rename-team" title="Rename team" prn={TEAM_PRN} slug="platform" name="Platform Team" action={action} />);

    const slug = screen.getByLabelText<HTMLInputElement>('Slug');
    const name = screen.getByLabelText<HTMLInputElement>('Name');
    expect(slug.value).toBe('platform');
    expect(name.value).toBe('Platform Team');
    expect(screen.getByRole('form', { name: 'Rename team' }).getAttribute('data-testid')).toBe('rename-team');

    await user.clear(slug);
    await user.type(slug, 'taken');
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await screen.findByText(FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT] ?? '');

    const posted = action.mock.calls[0]?.[1];
    expect(posted?.get('prn')).toBe(TEAM_PRN);
    expect(posted?.get('slug')).toBe('taken');
    expect(posted?.get('name')).toBe('Platform Team');
    expect(posted?.get('currentSlug')).toBe('platform');
    expect(posted?.get('currentName')).toBe('Platform Team');
  });

  it('keeps the typed values after an action that returns a failure', async () => {
    const user = userEvent.setup();
    renderInZone(<RenameForm testId="rename-team" title="Rename team" prn={TEAM_PRN} slug="platform" name="Platform Team" action={failing()} />);

    const slug = screen.getByLabelText<HTMLInputElement>('Slug');
    const name = screen.getByLabelText<HTMLInputElement>('Name');
    await user.clear(slug);
    await user.type(slug, 'taken');
    await user.clear(name);
    await user.type(name, 'Renamed Team');
    await user.click(screen.getByRole('button', { name: 'Rename' }));

    const error = await screen.findByText(FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT] ?? '');
    expect(screen.getByTestId('rename-team-error').contains(error)).toBe(true);
    expect(slug.value).toBe('taken');
    expect(name.value).toBe('Renamed Team');
  });

  // The Manage section's result region shows "Renamed." (spec § 6.4, manage-controls.test.tsx). The
  // form itself shows no success text, so the text never shows twice.
  it('shows no success text of its own after a success', async () => {
    const user = userEvent.setup();
    // Zero-arg implementation: TS allows assigning it where FormAction (2 args) is expected, and the
    // mock's calls are never inspected here, so no explicit arity type argument is needed.
    const action = vi.fn((): Promise<ActionState> => Promise.resolve({ ok: true }));
    renderInZone(<RenameForm testId="rename-team" title="Rename team" prn={TEAM_PRN} slug="platform" name="Platform Team" action={action} />);

    const button = screen.getByRole<HTMLButtonElement>('button', { name: 'Rename' });
    await user.click(button);

    await waitFor(() => {
      expect(action).toHaveBeenCalledTimes(1);
      expect(button.disabled).toBe(false);
    });
    expect(screen.queryByText('Renamed.')).toBeNull();
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.queryByTestId('form-error')).toBeNull();
  });

  // The control: the same wiring with an uncontrolled input LOSES the typed value. If this case
  // ever passes with the typed value, the harness no longer resets forms, and the case above proves
  // nothing.
  it('control: an uncontrolled input goes back to its default value after a failed action', async () => {
    const user = userEvent.setup();
    render(<UncontrolledControl action={failing()} />);

    const slug = screen.getByLabelText<HTMLInputElement>('Plain slug');
    await user.clear(slug);
    await user.type(slug, 'taken');
    await user.click(screen.getByRole('button', { name: 'Send' }));
    await screen.findByText('failed');

    expect(slug.value).toBe('platform');
  });
});
