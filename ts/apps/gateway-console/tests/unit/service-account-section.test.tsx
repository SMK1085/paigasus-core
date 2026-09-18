// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The service-accounts section (SMA-636 spec § 4.6, § 5.4, § 7.1). Each case renders the real
// section with real transitions and real form submissions, then renders it AGAIN with the props
// that the revalidated page sends — the technique of iam-console's manage-controls.test.tsx.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { FormAction } from '@paigasus/console-core';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY } from '../../app/_components/error-copy';
import { ServiceAccountSection } from '../../app/_components/service-account-section';
import type { ApiKeyRowView, IssueKeyState, SectionOk, SelectedView, ServiceAccountActions, ServiceAccountRowView } from '../../app/(console)/service-accounts/view';

vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/0190a100-0000-7000-8000-00000000000a' }));

afterEach(() => {
  cleanup();
});

const ORG_ID = '0190a100-0000-7000-8000-00000000000a';
const OWNER = `prn:pgs:iam:::organization/${ORG_ID}`;
const PATH = `/gateway/orgs/${ORG_ID}`;
const SA_ID = '0190a1e5-0000-7000-8000-0000000000a1';
const SA_PRN = `prn:pgs:iam:::principal/${SA_ID}`;
const TOKEN = 'pgs_unit_0123456789abcdef0123456789abcdef';

const ACCOUNT: ServiceAccountRowView = { prn: SA_PRN, id: SA_ID, name: 'ci-bot', created: '2026-09-18', active: true };
const ARCHIVED_ACCOUNT: ServiceAccountRowView = { ...ACCOUNT, active: false };
const KEY: ApiKeyRowView = { id: 'key-1', prefix: 'pgs_unit_old', status: 'active', created: '2026-09-18', expires: null, lastUsed: null, otherScope: null };
const ALL_CONTROLS = { allow: true, issue: true, revoke: true, archive: true };
const NO_CONTROLS = { allow: false, issue: false, revoke: false, archive: false };

function errorWith(presentation: Presentation, reason: ErrorReason | null = null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'corr-unit-sa',
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'grpc', code: 13, codeName: 'Internal' },
  };
}

function selected(overrides: Partial<Extract<SelectedView, { kind: 'ok' }>> = {}): SelectedView {
  return { kind: 'ok', account: ACCOUNT, modelCalls: 'no', keys: { kind: 'ok', rows: [KEY], page: { offset: 0, nextOffset: null } }, controls: ALL_CONTROLS, ...overrides };
}

function view(overrides: Partial<SectionOk> = {}): SectionOk {
  return {
    kind: 'ok',
    ownerPrn: OWNER,
    readOnly: null,
    rows: [ACCOUNT],
    page: { offset: 0, nextOffset: null },
    canCreate: true,
    selected: selected(),
    sa: SA_ID,
    saOffset: 0,
    keyOffset: 0,
    ...overrides,
  };
}

const ok: FormAction = () => Promise.resolve({ ok: true });

function actions(overrides: Partial<ServiceAccountActions> = {}): ServiceAccountActions {
  return {
    create: () => Promise.resolve({ kind: 'created', saPrn: SA_PRN, granted: true }),
    allow: ok,
    issue: () => Promise.resolve({ ok: true, token: TOKEN, prefix: 'pgs_unit_new' }),
    revoke: ok,
    archive: ok,
    ...overrides,
  };
}

function section(v: SectionOk, a: ServiceAccountActions): ReactNode {
  return (
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      <ServiceAccountSection key={v.ownerPrn} ownerKind="organization" path={PATH} view={v} actions={a} />
    </ZoneProvider>
  );
}

/** Renders the props of the revalidated page, with an awaited act() (see manage-controls.test.tsx). */
async function refresh(rerender: (ui: ReactNode) => void, ui: ReactNode): Promise<void> {
  await act(() => {
    rerender(ui);
    return Promise.resolve();
  });
}

const region = (): HTMLElement => screen.getByTestId('sa-result');
const panel = (): HTMLElement => screen.getByTestId('sa-panel');

async function revokeFirstKey(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  const row = within(panel()).getByTestId('api-key-row');
  await user.click(within(row).getByRole('button', { name: 'Revoke' }));
  await user.click(within(row).getByRole('button', { name: 'Confirm revoke' }));
}

describe('one result region: a result survives the refresh that unmounts its control (§ 4.6)', () => {
  it('keeps "Service account archived." after the archived account loses every control', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(section(view(), a));

    await user.click(within(panel()).getByRole('button', { name: 'Archive' }));
    await user.click(within(panel()).getByRole('button', { name: 'Confirm archive' }));
    await within(region()).findByText('Service account archived.');
    await refresh(rerender, section(view({ rows: [ARCHIVED_ACCOUNT], selected: selected({ account: ARCHIVED_ACCOUNT, modelCalls: 'archived', controls: NO_CONTROLS }) }), a));

    expect(within(panel()).queryByRole('button', { name: 'Archive' })).toBeNull();
    expect(within(region()).getByText('Service account archived.')).toBeDefined();
  });

  it('keeps "Key revoked." after the revoked key loses its Revoke button', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(section(view(), a));

    await revokeFirstKey(user);
    await within(region()).findByText('Key revoked.');
    await refresh(rerender, section(view({ selected: selected({ keys: { kind: 'ok', rows: [{ ...KEY, status: 'revoked' }], page: { offset: 0, nextOffset: null } } }) }), a));

    expect(within(panel()).queryByRole('button', { name: 'Revoke' })).toBeNull();
    expect(within(region()).getByText('Key revoked.')).toBeDefined();
  });

  it('keeps "Model calls allowed." after the Allow control goes away', async () => {
    const user = userEvent.setup();
    const a = actions();
    const { rerender } = render(section(view(), a));

    await user.click(within(panel()).getByRole('button', { name: 'Allow model calls' }));
    await within(region()).findByText('Model calls allowed.');
    await refresh(rerender, section(view({ selected: selected({ modelCalls: 'yes', controls: { ...ALL_CONTROLS, allow: false } }) }), a));

    expect(within(panel()).queryByRole('button', { name: 'Allow model calls' })).toBeNull();
    expect(within(region()).getByText('Model calls allowed.')).toBeDefined();
  });

  it('says "may already be allowed" for a generic answer to Allow (§ 5.3)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ allow: () => Promise.resolve({ ok: false, error: errorWith('generic') }) })));

    await user.click(within(panel()).getByRole('button', { name: 'Allow model calls' }));

    expect(await within(region()).findByText('Model calls may already be allowed. The page was reloaded.')).toBeDefined();
    expect(within(region()).getByTestId('correlation-id').textContent).toBe('corr-unit-sa');
  });
});

describe('the create result (§ 5.2)', () => {
  async function create(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    const form = screen.getByRole('form', { name: 'Create service account' });
    await user.type(within(form).getByLabelText('Name'), 'ci-bot');
    await user.click(within(form).getByRole('button', { name: 'Create' }));
  }

  it('says the account can call models, and links to select it', async () => {
    const user = userEvent.setup();
    render(section(view({ selected: { kind: 'none' }, sa: null }), actions()));

    await create(user);

    expect(await within(region()).findByText('Service account created. It can call models.')).toBeDefined();
    expect(within(region()).getByRole('link', { name: 'Select it' }).getAttribute('href')).toBe(`/orgs/${ORG_ID}?sa=${SA_ID}`);
  });

  it('says a partial create cannot call models yet, with the grant error and the select link', async () => {
    const user = userEvent.setup();
    render(section(view({ selected: { kind: 'none' }, sa: null }), actions({ create: () => Promise.resolve({ kind: 'partial', saPrn: SA_PRN, error: errorWith('generic') }) })));

    await create(user);

    expect(await within(region()).findByText('Service account created, but it cannot call models yet.')).toBeDefined();
    expect(within(region()).getByTestId('form-error')).toBeDefined();
    expect(within(region()).getByRole('link', { name: 'Select it' })).toBeDefined();
  });

  it('says a name conflict in its own words', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ create: () => Promise.resolve({ kind: 'failed', error: errorWith('conflict', ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT) }) })));

    await create(user);

    expect(await within(region()).findByText(FORM_REASON_COPY[ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT] ?? '')).toBeDefined();
  });
});

describe('the token (§ 5.4)', () => {
  it('calls the issue action directly with null, and keeps the token through the refresh (rules 2, 3)', async () => {
    const user = userEvent.setup();
    const issue = vi.fn<ServiceAccountActions['issue']>(() => Promise.resolve({ ok: true, token: TOKEN, prefix: 'pgs_unit_new' }));
    const a = actions({ issue });
    const { rerender } = render(section(view(), a));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));
    await waitFor(() => {
      expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    });
    expect(issue).toHaveBeenCalledTimes(1);
    expect(issue.mock.calls[0]?.[0]).toBeNull();
    expect(issue.mock.calls[0]?.[1].get('expiry')).toBe('90');
    expect(issue.mock.calls[0]?.[1].get('saPrn')).toBe(SA_PRN);

    const NEW_KEY: ApiKeyRowView = { ...KEY, id: 'key-2', prefix: 'pgs_unit_new' };
    await refresh(rerender, section(view({ selected: selected({ keys: { kind: 'ok', rows: [KEY, NEW_KEY], page: { offset: 0, nextOffset: null } } }) }), a));

    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });

  it('keeps the token panel open after a revoke, so key rotation works (rule 6)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions()));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));
    await waitFor(() => {
      expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    });
    await revokeFirstKey(user);
    await within(region()).findByText('Key revoked.');

    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });

  it('shows an issue result although a later action started and finished first (rule 4)', async () => {
    const user = userEvent.setup();
    let finishIssue: (state: IssueKeyState) => void = () => undefined;
    const issue = (): Promise<IssueKeyState> =>
      new Promise((resolve) => {
        finishIssue = resolve;
      });
    render(section(view(), actions({ issue })));
    const row = within(panel()).getByTestId('api-key-row');
    // Open the revoke confirmation BEFORE the issue starts.
    await user.click(within(row).getByRole('button', { name: 'Revoke' }));
    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));

    // Rule 5 disables the submit, so the later action is started past it, as a stale page could.
    fireEvent.submit(within(row).getByRole('form', { name: 'Confirm revoke' }));
    await within(region()).findByText('Key revoked.');
    await act(async () => {
      finishIssue({ ok: true, token: TOKEN, prefix: 'pgs_unit_new' });
      await Promise.resolve();
    });

    await waitFor(() => {
      expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    });
  });

  it('disables every other submit in the section while an issue is pending (rule 5)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ issue: () => new Promise<IssueKeyState>(() => undefined) })));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));

    await waitFor(() => {
      const enabled = screen.getAllByRole('button').filter((button) => !(button as HTMLButtonElement).disabled);
      expect(enabled.map((button) => button.textContent)).toEqual([]);
    });
  });

  it('says what a forbidden issue needs (§ 5.4)', async () => {
    const user = userEvent.setup();
    render(section(view(), actions({ issue: () => Promise.resolve<IssueKeyState>({ ok: false, error: errorWith('forbidden') }) })));

    await user.click(within(panel()).getByRole('button', { name: 'Issue key' }));

    expect(await within(region()).findByText('You need permission to issue keys here and to grant every role this account holds.')).toBeDefined();
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('renders no issue form and no create form in the server HTML (rule 7)', () => {
    const html = renderToStaticMarkup(section(view(), actions()));
    expect(html).toContain('data-testid="sa-panel"');
    expect(html).not.toContain('data-testid="issue-key-form"');
    expect(html).not.toContain('data-testid="sa-create-form"');
    expect(html).not.toContain('Allow model calls');
  });
});

describe('what the section shows', () => {
  it('says why an archived owner is read-only (§ 5.8)', () => {
    render(section(view({ readOnly: 'archived', canCreate: false, selected: selected({ controls: NO_CONTROLS }) }), actions()));
    expect(screen.getByTestId('sa-read-only').textContent).toBe('This organization is archived. IAM refuses changes to its service accounts and keys.');
    expect(screen.queryByRole('form', { name: 'Create service account' })).toBeNull();
  });

  it('says that an account of another scope is not shown (§ 4.2)', () => {
    render(section(view({ selected: { kind: 'other-scope' } }), actions()));
    expect(screen.getByTestId('sa-panel-other').textContent).toBe('This service account belongs to another scope.');
  });

  it('shows the key status labels and the note for a key of another scope (§ 5.7)', () => {
    const PROJECT = `prn:pgs:iam::${ORG_ID}:project/0190a1c3-0000-7000-8000-0000000000a1`;
    render(section(view({ selected: selected({ keys: { kind: 'ok', rows: [{ ...KEY, status: 'expired', otherScope: PROJECT }], page: { offset: 0, nextOffset: null } } }) }), actions()));
    const row = within(panel()).getByTestId('api-key-row');
    expect(row.getAttribute('data-status')).toBe('expired');
    expect(row.textContent).toContain('Expired');
    expect(within(row).getByTestId('api-key-scope').textContent).toBe(`Scope: ${PROJECT}. The gateway checks model calls against this scope.`);
    expect(within(row).queryByRole('button', { name: 'Revoke' })).toBeNull();
  });
});
