// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The result region of the Manage section (SMA-630 spec § 6.4). A rename, archive or restore action
// refreshes the page on a success and on `forbidden`/`conflict` (spec § 4.4). The refresh can change
// the lifecycle view or the rename key, and then the control that ran the action unmounts. The
// region keeps the last result through that refresh.
//
// Each case renders the real ManageSection with real useActionState and a real form submission,
// then renders it AGAIN with the props that the refreshed page sends.
import type { ReactNode } from 'react';
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { ActionState } from '@paigasus/console-core';
import { ErrorDomain, ErrorReason, type PaigasusError } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import type { FormAction } from '../../app/_components/form-action';
import { ManageSection, type ManageSectionProps } from '../../app/(console)/manage-section';
import type { NodeLifecycle } from '../../app/(console)/node-status';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs' }));

afterEach(() => {
  cleanup();
});

const TEAM_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000e1:team/0190a1b2-0000-7000-8000-0000000000e2';
const ACTIVE: NodeLifecycle = { own: 'active', effective: 'active' };
const ARCHIVED: NodeLifecycle = { own: 'archived', effective: 'archived' };
const ALL = { rename: true, archive: true, restore: true } as const;
const FORBIDDEN_COPY = FORM_REASON_COPY[ErrorReason.FORBIDDEN] ?? '';
const CONFLICT_COPY = FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT] ?? '';

function refusal(presentation: 'forbidden' | 'conflict', reason: ErrorReason, correlationId: string): PaigasusError {
  return {
    presentation,
    domain: ErrorDomain.IAM,
    reason,
    rawReason: presentation === 'forbidden' ? 'forbidden' : 'slug-conflict',
    rawDomain: 'iam.paigasus.io',
    message: 'IAM text that must never show',
    correlationId,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: presentation === 'forbidden' ? 403 : 409 },
  };
}

const FORBIDDEN = refusal('forbidden', ErrorReason.FORBIDDEN, 'corr-unit-manage-403');
const SLUG_CONFLICT = refusal('conflict', ErrorReason.SLUG_CONFLICT, 'corr-unit-manage-409');

function answering(state: ActionState): FormAction {
  return () => Promise.resolve(state);
}

type Props = Omit<ManageSectionProps, 'node' | 'prn'>;

function sectionAt(prn: string, props: Props): ReactNode {
  return (
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <ManageSection node="team" prn={prn} {...props} />
    </ZoneProvider>
  );
}

function section(props: Props): ReactNode {
  return sectionAt(TEAM_PRN, props);
}

function base(actions: Partial<Props['actions']>): Props {
  const noop = answering({ ok: true });
  return { name: 'Platform Team', slug: 'platform', lifecycle: ACTIVE, can: ALL, actions: { rename: noop, archive: noop, restore: noop, ...actions } };
}

async function confirmArchive(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  await user.click(screen.getByRole('button', { name: 'Archive' }));
  await user.click(screen.getByRole('button', { name: 'Confirm archive' }));
}

/** The text shows exactly once: the control and the region never repeat it. */
function expectOnce(text: string): void {
  expect(screen.getAllByText(text)).toHaveLength(1);
}

describe('ManageSection result region (spec § 6.4)', () => {
  it('keeps a refused archive, with its correlation id, after the refresh shows the node archived', async () => {
    const user = userEvent.setup();
    const props = base({ archive: answering({ ok: false, error: FORBIDDEN }) });
    const { rerender } = render(section(props));

    await confirmArchive(user);
    await within(screen.getByTestId('archive-team-error')).findByText(FORBIDDEN_COPY);
    // The error shows ONCE while the control that produced it is still mounted.
    expectOnce(FORBIDDEN_COPY);

    // Another user archived the team: the refreshed page shows Restore, and ArchiveButton unmounts.
    rerender(section({ ...props, lifecycle: ARCHIVED }));

    expect(screen.queryByTestId('archive-team')).toBeNull();
    expect(screen.getByTestId('restore-team')).toBeDefined();
    const region = screen.getByTestId('manage-result');
    expect(within(region).getByText(FORBIDDEN_COPY)).toBeDefined();
    expect(within(region).getByTestId('correlation-id').textContent).toBe('corr-unit-manage-403');
    expectOnce(FORBIDDEN_COPY);
  });

  it('keeps a refused rename after the refresh shows a new slug and name (the rename key changes)', async () => {
    const user = userEvent.setup();
    const props = base({ rename: answering({ ok: false, error: SLUG_CONFLICT }) });
    const { rerender } = render(section(props));

    await user.clear(screen.getByLabelText('Slug'));
    await user.type(screen.getByLabelText('Slug'), 'taken');
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await within(screen.getByTestId('rename-team-error')).findByText(CONFLICT_COPY);
    expectOnce(CONFLICT_COPY);

    // Another user renamed the team: the form starts again from the new values.
    rerender(section({ ...props, slug: 'platform-2', name: 'Platform Two' }));

    expect(screen.getByLabelText<HTMLInputElement>('Slug').value).toBe('platform-2');
    const region = screen.getByTestId('manage-result');
    expect(within(region).getByText(CONFLICT_COPY)).toBeDefined();
    expect(within(region).getByTestId('correlation-id').textContent).toBe('corr-unit-manage-409');
    expectOnce(CONFLICT_COPY);
  });

  it('shows "Archived." after a successful archive and the refresh', async () => {
    const user = userEvent.setup();
    const props = base({ archive: answering({ ok: true }) });
    const { rerender } = render(section(props));

    await confirmArchive(user);
    await within(screen.getByTestId('manage-result')).findByText('Archived.');
    rerender(section({ ...props, lifecycle: ARCHIVED }));

    expect(screen.getByTestId('manage-result').textContent).toBe('Archived.');
    expectOnce('Archived.');
  });

  it('shows "Restored." after a successful restore and the refresh', async () => {
    const user = userEvent.setup();
    const props = { ...base({ restore: answering({ ok: true }) }), lifecycle: ARCHIVED };
    const { rerender } = render(section(props));

    await user.click(screen.getByRole('button', { name: 'Restore' }));
    await within(screen.getByTestId('manage-result')).findByText('Restored.');
    rerender(section({ ...props, lifecycle: ACTIVE }));

    expect(screen.queryByTestId('restore-team')).toBeNull();
    expect(screen.getByTestId('manage-result').textContent).toBe('Restored.');
    expectOnce('Restored.');
  });

  it('shows "Renamed." after a successful rename and the refresh', async () => {
    const user = userEvent.setup();
    const props = base({ rename: answering({ ok: true }) });
    const { rerender } = render(section(props));

    await user.clear(screen.getByLabelText('Slug'));
    await user.type(screen.getByLabelText('Slug'), 'platform-2');
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await within(screen.getByTestId('manage-result')).findByText('Renamed.');
    rerender(section({ ...props, slug: 'platform-2' }));

    expect(screen.getByTestId('manage-result').textContent).toBe('Renamed.');
    expectOnce('Renamed.');
  });

  it('replaces the previous result when the user submits again', async () => {
    const user = userEvent.setup();
    const props = base({ archive: answering({ ok: false, error: FORBIDDEN }), restore: answering({ ok: true }) });
    const { rerender } = render(section(props));

    await confirmArchive(user);
    await within(screen.getByTestId('archive-team-error')).findByText(FORBIDDEN_COPY);
    rerender(section({ ...props, lifecycle: ARCHIVED }));
    expect(within(screen.getByTestId('manage-result')).getByText(FORBIDDEN_COPY)).toBeDefined();

    await user.click(screen.getByRole('button', { name: 'Restore' }));

    await within(screen.getByTestId('manage-result')).findByText('Restored.');
    expect(screen.queryByText(FORBIDDEN_COPY)).toBeNull();
    expect(screen.queryByTestId('correlation-id')).toBeNull();
  });

  it('keeps the typed values next to the error after a refused rename', async () => {
    const user = userEvent.setup();
    render(section(base({ rename: answering({ ok: false, error: SLUG_CONFLICT }) })));

    await user.clear(screen.getByLabelText('Slug'));
    await user.type(screen.getByLabelText('Slug'), 'taken');
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await within(screen.getByTestId('rename-team-error')).findByText(CONFLICT_COPY);

    expect(screen.getByLabelText<HTMLInputElement>('Slug').value).toBe('taken');
    expectOnce(CONFLICT_COPY);
  });

  it('removes the error of another control when a new submission succeeds', async () => {
    const user = userEvent.setup();
    const props = base({ rename: answering({ ok: false, error: SLUG_CONFLICT }), archive: answering({ ok: true }) });
    const { rerender } = render(section(props));

    await user.clear(screen.getByLabelText('Slug'));
    await user.type(screen.getByLabelText('Slug'), 'taken');
    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await within(screen.getByTestId('rename-team-error')).findByText(CONFLICT_COPY);

    await confirmArchive(user);
    await within(screen.getByTestId('manage-result')).findByText('Archived.');
    rerender(section({ ...props, lifecycle: ARCHIVED }));

    expect(screen.queryByText(CONFLICT_COPY)).toBeNull();
    expect(screen.getByTestId('rename-team-error').textContent).toBe('');
    expect(screen.getByTestId('manage-result').textContent).toBe('Archived.');
    expectOnce('Archived.');
  });

  it('ignores the result of a submission that a later submission replaced', async () => {
    const user = userEvent.setup();
    let finishRename: (state: ActionState) => void = () => undefined;
    const slowRename: FormAction = () =>
      new Promise<ActionState>((resolve) => {
        finishRename = resolve;
      });
    const props = base({ rename: slowRename, archive: answering({ ok: true }) });
    const { rerender } = render(section(props));

    await user.click(screen.getByRole('button', { name: 'Rename' }));
    await confirmArchive(user);
    // The rename finishes AFTER the archive.
    await act(async () => {
      finishRename({ ok: true });
      await Promise.resolve();
    });
    await waitFor(() => {
      expect(screen.getByTestId('manage-result').textContent).toBe('Archived.');
    });
    rerender(section({ ...props, lifecycle: ARCHIVED }));

    expect(screen.getByTestId('manage-result').textContent).toBe('Archived.');
    expect(screen.queryByText('Renamed.')).toBeNull();
    expectOnce('Archived.');
  });

  it('empties the result region when the user moves to a different node (CR round 1)', async () => {
    const user = userEvent.setup();
    const props = base({ archive: answering({ ok: true }) });
    const { rerender } = render(section(props));

    await confirmArchive(user);
    await within(screen.getByTestId('manage-result')).findByText('Archived.');

    // A client navigation to a different node's page: a new PRN, name and slug, the same lifecycle
    // view. Without a key on ManageControls, React reuses the instance and node A's result shows on
    // node B (spec § 6.4).
    const OTHER_PRN = 'prn:pgs:iam::0190a100-0000-7000-8000-0000000000e1:team/0190a1b2-0000-7000-8000-0000000000f9';
    rerender(sectionAt(OTHER_PRN, { ...props, name: 'Other Team', slug: 'other' }));

    expect(screen.getByTestId('manage-result').textContent).toBe('');
    expect(screen.queryByText('Archived.')).toBeNull();
  });
});
