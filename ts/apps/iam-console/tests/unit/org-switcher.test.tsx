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

/**
 * The rendered markup's text, tags removed. The switcher's "Organization: " prefix is screen-reader
 * text in its own span beside a decorative icon, so it is not contiguous with the label in markup.
 */
function textOf(markup: string): string {
  return markup.replace(/<[^>]*>/g, '');
}

function renderShell(): string {
  return renderToStaticMarkup(
    <ZoneProvider zone="iam" zones={{ iam: '/iam' }}>
      <SessionProvider value={{ principalPrn: null, displayName: 'Ada', email: null, grants: [], grantsAvailable: false }}>
        <OrgSwitcherShell brand={{ label: 'IAM', href: '/iam/orgs' }} nav={[]} consoles={[]} orgs={ORGS}>
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
    expect(textOf(renderShell())).toContain('Organization: Globex');
  });

  it('shows the current organization as an initials badge, hidden from assistive tech, in place of the visible prefix', () => {
    params.current = { org: GLOBEX };
    const markup = renderShell();
    expect(markup).toMatch(/<span aria-hidden="true"[^>]*>G<\/span>/);
    expect(markup).toContain('<span class="sr-only">Organization:</span> <span class="max-w-48 truncate max-md:sr-only">Globex</span>');
  });

  it('falls back to the organization icon while no organization is selected', () => {
    params.current = {};
    const markup = renderShell();
    expect(markup).toMatch(/<span aria-hidden="true"[^>]*><svg /);
  });

  it('reads an upper-case [org] segment as the lower-case id of its item, and marks it as current', () => {
    expect(currentOrgId({ org: GLOBEX.toUpperCase() })).toBe(GLOBEX);
    params.current = { org: GLOBEX.toUpperCase() };
    expect(textOf(renderShell())).toContain('Organization: Globex');
  });

  it('marks nothing on a page outside one organization', () => {
    params.current = {};
    expect(textOf(renderShell())).toContain('Organization: none selected');
  });
});
