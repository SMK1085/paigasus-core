// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// A minted key whose token cannot reach the TokenPanel (SMA-636 spec § 5.4 rule 4). IAM minted the
// key, so the user must learn about it. The frame says so loudly in the result region, with the key
// prefix, instead of dropping the token without a word. The TokenPanel here is a double that never
// attaches its handle, which is the one way the frame's `show()` can fail to reach it.
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactElement, ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ZoneProvider } from '@paigasus/app-shell';
import { serviceAccountsBlock } from '../../app/(console)/service-accounts/block';
import type { SectionOk } from '../../app/(console)/service-accounts/view';

vi.mock('../../app/_components/token-panel', () => ({
  TokenPanel: (): ReactElement | null => null,
}));
vi.mock('../../app/(console)/service-accounts/actions', () => ({
  createServiceAccountAction: () => Promise.resolve(null),
  allowModelCallsAction: () => Promise.resolve({ ok: true }),
  issueApiKeyAction: () => Promise.resolve({ ok: true, token: 'pgs_unit_lost_0123456789abcdef0123456789ab', prefix: 'pgs_unit_lost' }),
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
const SA_ID = '0190a1e5-0000-7000-8000-0000000000a1';
const SA_PRN = `prn:pgs:iam:::principal/${SA_ID}`;
const ACCOUNT = { prn: SA_PRN, id: SA_ID, name: 'ci-bot', created: '2026-09-18', active: true };

const VIEW: SectionOk = {
  kind: 'ok',
  ownerPrn: OWNER,
  readOnly: null,
  rows: [ACCOUNT],
  page: { offset: 0, nextOffset: null },
  canCreate: true,
  selected: {
    kind: 'ok',
    account: ACCOUNT,
    modelCalls: 'yes',
    keys: { kind: 'ok', rows: [], page: { offset: 0, nextOffset: null } },
    controls: { allow: false, issue: true, revoke: true, archive: true },
  },
  sa: SA_ID,
  saOffset: 0,
  keyOffset: 0,
};

describe('a token that cannot reach the TokenPanel', () => {
  it('says loudly that a key was issued, and names its prefix', async () => {
    const user = userEvent.setup();
    render(
      <ZoneProvider zone="gateway" zones={{ gateway: '/gateway' }}>
        {await serviceAccountsBlock({ view: VIEW, ownerKind: 'organization', ownerPrn: OWNER, path: `/gateway/orgs/${ORG_ID}` })}
      </ZoneProvider>,
    );

    await user.click(within(screen.getByTestId('sa-panel')).getByRole('button', { name: 'Issue key' }));

    const alert = await within(screen.getByTestId('sa-result')).findByTestId('token-lost');
    expect(alert.getAttribute('role')).toBe('alert');
    expect(alert.textContent).toBe('A key was issued but its token could not be shown. Revoke key pgs_unit_lost.');
    expect(document.body.textContent).not.toContain('pgs_unit_lost_0123456789abcdef');
  });
});
