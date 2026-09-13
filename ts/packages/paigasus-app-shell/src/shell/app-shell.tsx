// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement, ReactNode } from 'react';
import { cn } from '@paigasus/ui';
import { PrimaryNav, type NavEntry } from '../nav/primary-nav';
import { ZoneLink } from '../zone/zone-link';
import { MAIN_ID, SkipLink } from './skip-link';
import { Switcher, type SwitcherProps } from './switcher';
import { UserMenu } from './user-menu';

/*
 * Sentinel C (SMA-511 spec § 7.6). SOURCE_PROBE's value, below, is an arbitrary custom-property
 * utility that exists ONLY there, and it compiles to a single declaration in the built CSS.
 * ci/tailwind-source/run.mjs asserts it reaches the iam-console production build, which is what
 * proves the app's Tailwind `@source` line still covers this package.
 *
 * It follows @paigasus/ui's sentinel A (src/components/table.tsx). Do not remove it, do not rename
 * it, and do not write its literal value anywhere else in this file or under ts/apps/ — Tailwind's
 * scanner reads raw file text, comments included, so a second copy (even in prose) could generate
 * the utility independently and silently disarm the gate. That is why this comment does not spell
 * out the literal itself; see the assignment below, or run.mjs's PROBE_APP_SHELL.
 */
const SOURCE_PROBE = '[--paigasus-app-shell-source-probe:1]';

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
      <header className={cn(SOURCE_PROBE, 'flex flex-wrap items-center gap-4 border-b border-border bg-background px-4 py-2')}>
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
