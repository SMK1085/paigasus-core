// SPDX-License-Identifier: Apache-2.0
'use client';

import { useEffect, useId, useRef, type ReactElement, type ReactNode } from 'react';
import { useSession } from '@paigasus/auth/client';
import { cn } from '@paigasus/ui';
import { PrimaryNav, type NavEntry } from '../nav/primary-nav';
import { navRows } from '../nav/rows';
import { useZone } from '../zone/context';
import { BrandLink, type Brand } from './brand-link';
import { ConsoleSwitcher } from './console-switcher';
import { sidebarClasses, useSidebar } from './sidebar';
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

export type { Brand };

export type AppShellProps = {
  readonly brand: Brand;
  /** The CURRENT console's pages, optionally grouped (`group`). Another zone's entry throws. */
  readonly nav: readonly NavEntry[];
  /**
   * Every console the user can switch to, the current one included (SMA-783). With two or more left
   * after the entry rules, the brand becomes the console switcher; otherwise it is a plain link.
   */
  readonly consoles?: readonly NavEntry[];
  /** One Switcher per level (org, team). */
  readonly switchers?: readonly SwitcherProps[];
  readonly children: ReactNode;
};

/**
 * The authenticated shell (spec § 8.1). It must be rendered under BOTH ZoneProvider and
 * SessionProvider. UserMenu and PrimaryNav call useSession(), so without a SessionProvider this
 * throws, which makes a missing provider loud. It takes no breadcrumbs: a page renders its own.
 */
/* Lucide's `panel-left` (ISC), inline like the shell's other glyphs. Decorative. */
function PanelLeftGlyph(): ReactElement {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      className="size-4"
    >
      <rect width="18" height="18" x="3" y="3" rx="2" />
      <path d="M9 3v18" />
    </svg>
  );
}

export function AppShell({ brand, nav, consoles, switchers, children }: AppShellProps): ReactElement {
  const session = useSession();
  const { zone, zones } = useZone();
  const sidebar = useSidebar();
  const sidebarId = useId();
  const toggleRef = useRef<HTMLButtonElement>(null);
  const sidebarRef = useRef<HTMLDivElement>(null);
  // The toggle exists only when the sidebar would show something (PrimaryNav's own rules).
  const hasNav = navRows('PrimaryNav', nav, zones, session, zone).length > 0;

  // Narrow screens only, where the open sidebar covers main: Escape inside it closes it and returns
  // focus to the toggle, and following one of its links closes it so the page is visible. Native
  // listeners on the wrapper, which only delegates: it is not itself an interactive element.
  const { wide, close } = sidebar;
  useEffect(() => {
    const element = sidebarRef.current;
    if (wide || element === null) return;
    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.key !== 'Escape') return;
      close();
      toggleRef.current?.focus();
    };
    const onClick = (event: MouseEvent): void => {
      if (event.target instanceof Element && event.target.closest('a[href]') !== null) close();
    };
    element.addEventListener('keydown', onKeyDown);
    element.addEventListener('click', onClick);
    return () => {
      element.removeEventListener('keydown', onKeyDown);
      element.removeEventListener('click', onClick);
    };
  }, [wide, close]);

  return (
    <>
      <SkipLink />
      <div className="flex min-h-dvh flex-col">
        <header className={cn(SOURCE_PROBE, 'flex flex-wrap items-center gap-4 border-b border-border bg-background px-4 py-2')}>
          {hasNav ? (
            <button
              ref={toggleRef}
              type="button"
              aria-expanded={sidebar.open}
              aria-controls={sidebarId}
              onClick={sidebar.toggle}
              className="-ml-1 inline-flex size-8 shrink-0 items-center justify-center rounded-pgs text-muted-foreground outline-hidden hover:bg-muted hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring"
            >
              <PanelLeftGlyph />
              <span className="sr-only">Navigation</span>
            </button>
          ) : null}
          {consoles === undefined ? <BrandLink brand={brand} /> : <ConsoleSwitcher brand={brand} consoles={consoles} />}
          <div className="ml-auto flex items-center gap-2">
            {switchers?.map((switcher) => (
              <Switcher
                key={switcher.label}
                label={switcher.label}
                items={switcher.items}
                currentId={switcher.currentId}
                {...(switcher.icon !== undefined ? { icon: switcher.icon } : {})}
                {...(switcher.initials !== undefined ? { initials: switcher.initials } : {})}
              />
            ))}
            <UserMenu />
          </div>
        </header>
        {/* The Primary nav is a left sidebar beside main, not part of the banner. It sizes and borders
          itself, so a shell whose nav has no visible entry (PrimaryNav returns null) has no empty column. */}
        <div className="relative flex flex-1">
          {/* Not a landmark of its own: PrimaryNav inside is the "Primary" navigation landmark. */}
          <div ref={sidebarRef} id={sidebarId} className={sidebarClasses(sidebar.choice)}>
            <PrimaryNav entries={nav} />
          </div>
          <main id={MAIN_ID} tabIndex={-1} className="min-w-0 flex-1 outline-none">
            {children}
          </main>
        </div>
      </div>
    </>
  );
}
