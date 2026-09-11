// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement, ReactNode } from 'react';
import { useZone } from '../zone/context';
import { ZoneLink } from '../zone/zone-link';
import type { Brand } from './app-shell';
import { MAIN_ID, SkipLink } from './skip-link';

export type PublicShellProps = {
  readonly brand: Brand;
  readonly children: ReactNode;
};

/**
 * The shell for a visitor with no session (spec § 8.2, AC 4): a signed-out landing page, the page
 * after logout, error pages. It needs a ZoneProvider and NO SessionProvider: it never calls
 * useSession(), and it renders no nav, no switcher and no user menu.
 *
 * "Sign in" is a plain <a>, never next/link: login is a route handler that rejects any request
 * whose Sec-Fetch-Mode is not `navigate` (F8). It carries no returnTo; for a protected page the
 * middleware redirect already adds one.
 */
export function PublicShell({ brand, children }: PublicShellProps): ReactElement {
  const { basePath } = useZone();
  return (
    <>
      <SkipLink />
      <header className="flex items-center gap-4 border-b border-border bg-background px-4 py-2">
        <ZoneLink href={brand.href} className="font-semibold">
          {brand.label}
        </ZoneLink>
        <a href={`${basePath}/auth/login`} className="ml-auto text-sm hover:underline">
          Sign in
        </a>
      </header>
      <main id={MAIN_ID} tabIndex={-1} className="outline-none">
        {children}
      </main>
    </>
  );
}
