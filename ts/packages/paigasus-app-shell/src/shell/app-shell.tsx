// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement, ReactNode } from 'react';
import { PrimaryNav, type NavEntry } from '../nav/primary-nav';
import { ZoneLink } from '../zone/zone-link';
import { MAIN_ID, SkipLink } from './skip-link';
import { Switcher, type SwitcherProps } from './switcher';
import { UserMenu } from './user-menu';

export type Brand = {
  readonly label: string;
  /** A full path (spec § 6.2). */
  readonly href: string;
};

export type AppShellProps = {
  readonly brand: Brand;
  readonly nav: readonly NavEntry[];
  /** One Switcher per level (org, team). */
  readonly switchers?: readonly SwitcherProps[];
  readonly children: ReactNode;
};

/**
 * The authenticated shell (spec § 8.1). It must be rendered under BOTH ZoneProvider and
 * SessionProvider. UserMenu and PrimaryNav call useSession(), so without a SessionProvider this
 * throws, which makes a missing provider loud. It takes no breadcrumbs: a page renders its own.
 */
export function AppShell({ brand, nav, switchers, children }: AppShellProps): ReactElement {
  return (
    <>
      <SkipLink />
      <header className="flex flex-wrap items-center gap-4 border-b border-border bg-background px-4 py-2">
        <ZoneLink href={brand.href} className="font-semibold">
          {brand.label}
        </ZoneLink>
        <PrimaryNav entries={nav} />
        <div className="ml-auto flex items-center gap-2">
          {switchers?.map((switcher) => (
            <Switcher key={switcher.label} label={switcher.label} items={switcher.items} currentId={switcher.currentId} />
          ))}
          <UserMenu />
        </div>
      </header>
      <main id={MAIN_ID} tabIndex={-1} className="outline-none">
        {children}
      </main>
    </>
  );
}
