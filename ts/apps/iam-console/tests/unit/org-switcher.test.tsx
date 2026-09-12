// SPDX-License-Identifier: Apache-2.0
//
// The organization switcher wrapper (spec § 5.4): it reads the `[org]` segment with useParams()
// and marks that organization as current. The selection lives in the URL only.
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { SessionProvider } from '@paigasus/auth/client';
import { ZoneProvider } from '@paigasus/app-shell';

const params: { current: Record<string, string | string[]> } = vi.hoisted(() => ({ current: {} }));
vi.mock('next/navigation', async (importOriginal) => ({
  ...(await importOriginal<typeof import('next/navigation')>()),
  useParams: () => params.current,
  usePathname: () => '/orgs',
}));
vi.mock('next/link', () => ({
  default: ({ href, children }: { href: string; children?: ReactNode }) => <a href={href}>{children}</a>,
}));

const { currentOrgId, orgSwitcherItems, OrgSwitcherShell } = await import('../../app/_components/org-switcher');

const ACME = '0192f1c0-0000-7000-8000-00000000000a';
const GLOBEX = '0192f1c0-0000-7000-8000-00000000000b';
const ORGS = [
  { orgId: ACME, label: 'Acme' },
  { orgId: GLOBEX, label: 'Globex' },
];

function renderShell(): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <SessionProvider value={{ principalPrn: null, displayName: 'Ada', email: null, grants: [], grantsAvailable: false }}>
        <OrgSwitcherShell brand={{ label: 'Paigasus IAM', href: '/iam/orgs' }} nav={[]} orgs={ORGS}>
          <p>page</p>
        </OrgSwitcherShell>
      </SessionProvider>
    </ZoneProvider>,
  );
}

describe('the organization switcher', () => {
  it('builds full-path hrefs under the basePath', () => {
    expect(orgSwitcherItems(ORGS, '/iam')).toEqual([
      { id: ACME, label: 'Acme', href: `/iam/orgs/${ACME}` },
      { id: GLOBEX, label: 'Globex', href: `/iam/orgs/${GLOBEX}` },
    ]);
  });

  it('reads the current organization from the [org] segment only', () => {
    expect(currentOrgId({ org: ACME })).toBe(ACME);
    expect(currentOrgId({ org: [ACME] })).toBeNull();
    expect(currentOrgId({})).toBeNull();
    expect(currentOrgId(null)).toBeNull();
  });

  it('marks the organization in the URL as current', () => {
    params.current = { org: GLOBEX };
    expect(renderShell()).toContain('Organization: Globex');
  });

  it('reads an upper-case [org] segment as the lower-case id of its item, and marks it as current', () => {
    expect(currentOrgId({ org: GLOBEX.toUpperCase() })).toBe(GLOBEX);
    params.current = { org: GLOBEX.toUpperCase() };
    expect(renderShell()).toContain('Organization: Globex');
  });

  it('marks nothing on a page outside one organization', () => {
    params.current = {};
    expect(renderShell()).toContain('Organization: none selected');
  });
});
