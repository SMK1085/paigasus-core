// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';

/*
 * The organisation switcher's icon (design system: Switcher, "Icon, not a prefix"). Lucide's
 * `building-2` (ISC), drawn inline rather than imported: lucide-react is not a dependency, and the
 * one other glyph in the console (Dialog's close button) is inline the same way. Decorative only —
 * Switcher wraps it in an aria-hidden span and keeps its label as screen-reader text — so it takes
 * no title. `currentColor` lets the wrapper's text-muted-foreground set its colour.
 */
export function OrganizationIcon(): ReactElement {
  return (
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" focusable="false">
      <path d="M6 22V4a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v18Z" />
      <path d="M6 12H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h2" />
      <path d="M18 9h2a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-2" />
      <path d="M10 6h4" />
      <path d="M10 10h4" />
      <path d="M10 14h4" />
      <path d="M10 18h4" />
    </svg>
  );
}
