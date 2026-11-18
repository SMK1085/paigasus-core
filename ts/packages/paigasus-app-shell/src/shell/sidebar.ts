// SPDX-License-Identifier: Apache-2.0
//
// The sidebar's open state (SMA-783). A plain module, not a component: no 'use client'
// (tests/structure/source-shape.test.ts); AppShell, a client component, is its one caller.
import { useCallback, useState, useSyncExternalStore } from 'react';

/** Tailwind's `md` breakpoint. Below it the sidebar starts collapsed and opens as an overlay. */
export const WIDE_QUERY = '(min-width: 48rem)';

function mediaQuery(): MediaQueryList | null {
  // jsdom and some embedded browsers have no matchMedia: treat them as wide, the default layout.
  return typeof window === 'undefined' || typeof window.matchMedia !== 'function' ? null : window.matchMedia(WIDE_QUERY);
}

function subscribe(onChange: () => void): () => void {
  const query = mediaQuery();
  query?.addEventListener('change', onChange);
  return () => query?.removeEventListener('change', onChange);
}

const isWide = (): boolean => mediaQuery()?.matches ?? true;
// The server cannot know the viewport. It renders the wide answer, and the sidebar's markup leaves
// the choice to CSS (`hidden md:flex`) until the user picks, so a narrow screen never flashes it.
const isWideOnServer = (): boolean => true;

export type SidebarChoice = 'open' | 'closed' | null;

export type Sidebar = {
  /** At or above the `md` breakpoint. */
  readonly wide: boolean;
  /** The user's choice this session, or null: open when wide, collapsed when narrow. */
  readonly choice: SidebarChoice;
  readonly open: boolean;
  readonly toggle: () => void;
  readonly close: () => void;
};

/**
 * Open by default on a wide screen, collapsed by default on a narrow one; the toggle overrides
 * either for the rest of the page's life. The choice is not stored: a full page load (another
 * console, a reload) starts from the default again.
 */
export function useSidebar(): Sidebar {
  const wide = useSyncExternalStore(subscribe, isWide, isWideOnServer);
  const [choice, setChoice] = useState<SidebarChoice>(null);
  const open = choice === null ? wide : choice === 'open';
  // Stable, so an effect can depend on it without re-subscribing every render.
  const close = useCallback(() => {
    setChoice('closed');
  }, []);
  return {
    wide,
    choice,
    open,
    toggle: () => {
      setChoice(open ? 'closed' : 'open');
    },
    close,
  };
}

/** The sidebar wrapper's visibility classes for a choice. */
export function sidebarClasses(choice: SidebarChoice): string {
  if (choice === null) return 'hidden md:flex';
  if (choice === 'closed') return 'hidden';
  // Open: beside main when wide; over main, below the header, when narrow.
  return 'flex max-md:absolute max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:bg-background max-md:shadow-lg';
}
