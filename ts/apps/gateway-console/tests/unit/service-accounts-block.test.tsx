// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The service-accounts BLOCK (SMA-636 spec § 4.6, § 5.4 rules 3 and 6). A successful issue
// revalidates the page (plan SPEC DEVIATION 8). The revalidated render can turn the section into a
// SectionError or a denial. The token must survive that: the block renders the result region and
// the TokenPanel in one client frame for EVERY section view kind, keyed by the owner PRN.
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { serviceAccountsBlock } from '../../app/(console)/service-accounts/block';
import type { SectionOk, SectionView } from '../../app/(console)/service-accounts/view';

const TOKEN = 'pgs_unit_block_0123456789abcdef0123456789ab';

// The real actions module opens lib/console, which needs a request scope. The block only passes
// the actions through, so doubles are enough here.
vi.mock('../../app/(console)/service-accounts/actions', () => ({
  createServiceAccountAction: () => Promise.resolve(null),
  allowModelCallsAction: () => Promise.resolve({ ok: true }),
  issueApiKeyAction: () => Promise.resolve({ ok: true, token: 'pgs_unit_block_0123456789abcdef0123456789ab', prefix: 'pgs_unit_blk' }),
  revokeApiKeyAction: () => Promise.resolve({ ok: true }),
  archiveServiceAccountAction: () => Promise.resolve({ ok: true }),
}));
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));
vi.mock('next/navigation', async (importOriginal) => ({ ...(await importOriginal<typeof import('next/navigation')>()), usePathname: () => '/orgs/0190a100-0000-7000-8000-00000000000a' }));

afterEach(() => {
  cleanup();
});

const ORG_ID = '0190a100-0000-7000-8000-00000000000a';
const OWNER = `prn:pgs:iam:::organization/${ORG_ID}`;
const OTHER_OWNER = 'prn:pgs:iam:::organization/0190a100-0000-7000-8000-00000000000b';
const PATH = `/gateway/orgs/${ORG_ID}`;
const SA_ID = '0190a1e5-0000-7000-8000-0000000000a1';
const SA_PRN = `prn:pgs:iam:::principal/${SA_ID}`;

const DEGRADED: PaigasusError = {
  presentation: 'degraded',
  domain: null,
  reason: null,
  rawReason: null,
  rawDomain: null,
  message: 'IAM text that must never show',
  correlationId: 'corr-unit-block',
  requestId: null,
  retryable: true,
  metadata: {},
  transport: { kind: 'grpc', code: 14, codeName: 'Unavailable' },
};

const OK_VIEW: SectionOk = {
  kind: 'ok',
  ownerPrn: OWNER,
  readOnly: null,
  rows: [{ prn: SA_PRN, id: SA_ID, name: 'ci-bot', created: '2026-09-18', active: true }],
  page: { offset: 0, nextOffset: null },
  canCreate: true,
  selected: {
    kind: 'ok',
    account: { prn: SA_PRN, id: SA_ID, name: 'ci-bot', created: '2026-09-18', active: true },
    modelCalls: 'yes',
    keys: { kind: 'ok', rows: [], page: { offset: 0, nextOffset: null } },
    controls: { allow: false, issue: true, revoke: true, archive: true },
  },
  sa: SA_ID,
  saOffset: 0,
  keyOffset: 0,
};

async function block(view: SectionView, ownerPrn: string = OWNER): Promise<ReactElement> {
  return (
    <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
      {await serviceAccountsBlock({ view, ownerKind: 'organization', ownerPrn, path: PATH })}
    </ZoneProvider>
  );
}

async function issueKey(): Promise<void> {
  const user = userEvent.setup();
  await user.click(within(screen.getByTestId('sa-panel')).getByRole('button', { name: 'Issue key' }));
  await waitFor(() => {
    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });
}

async function refresh(rerender: (ui: ReactNode) => void, ui: ReactElement): Promise<void> {
  await act(() => {
    rerender(ui);
    return Promise.resolve();
  });
}

describe('serviceAccountsBlock keeps the token when the revalidated section fails (§ 5.4 rule 3)', () => {
  it('keeps the token when the section turns into a SectionError', async () => {
    const { rerender } = render(await block(OK_VIEW));
    await issueKey();

    await refresh(rerender, await block({ kind: 'error', error: DEGRADED }));

    expect(screen.getByTestId('section-error')).toBeDefined();
    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });

  it('keeps the token when the section turns into a denial', async () => {
    const { rerender } = render(await block(OK_VIEW));
    await issueKey();

    await refresh(rerender, await block({ kind: 'denied' }));

    expect(screen.getByTestId('service-accounts-denied')).toBeDefined();
    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
  });

  it('starts an empty frame for another owner', async () => {
    const { rerender } = render(await block(OK_VIEW));
    await issueKey();

    await refresh(rerender, await block({ ...OK_VIEW, ownerPrn: OTHER_OWNER }, OTHER_OWNER));

    expect(screen.queryByTestId('token-value')).toBeNull();
  });
});
