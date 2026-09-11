// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';

/** The id of the <main> element that both shells render and the skip link targets. */
export const MAIN_ID = 'main';

/** "Skip to content". Visually hidden until it has focus. A plain in-page anchor, never next/link. */
export function SkipLink(): ReactElement {
  return (
    <a href={`#${MAIN_ID}`} className="sr-only focus:not-sr-only focus:absolute focus:left-2 focus:top-2 focus:rounded-pgs focus:bg-background focus:px-2 focus:py-1">
      Skip to content
    </a>
  );
}
