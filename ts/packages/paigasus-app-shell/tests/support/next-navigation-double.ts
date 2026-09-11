// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/navigation` in the jsdom tier. `usePathname()` returns the pathname WITHOUT
// the base path, as the real hook does (spec F19, measured as E7). Install it with:
//   vi.mock('next/navigation', () => import('../support/next-navigation-double'));
let current = '/';

/** Set what usePathname() returns. Call it before render. */
export function setPathname(pathname: string): void {
  current = pathname;
}

export function usePathname(): string {
  return current;
}
