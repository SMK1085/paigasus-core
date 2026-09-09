// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';

/*
 * Hand-written, not shadcn-generated — see dialog.tsx and the package README for why. Plain
 * composition, no radix primitive needed. `retry` is imperative (re-run the failed request),
 * so it renders as a real `<button>` wired to `onClick` rather than a `Link` — a retry is not
 * navigation, and treating it as one would give it the wrong semantics for assistive tech.
 */

export interface ErrorStateProps {
  title: string;
  description?: string;
  retry?: () => void;
}

export function ErrorState({ title, description, retry }: ErrorStateProps): ReactElement {
  return (
    <div className="border-destructive/40 flex flex-col items-center gap-2 border p-8 text-center rounded-pgs">
      <h2 className="text-destructive text-sm font-medium">{title}</h2>
      {description === undefined ? null : <p className="text-muted-foreground text-sm">{description}</p>}
      {retry === undefined ? null : (
        <button
          type="button"
          onClick={retry}
          className="bg-primary text-primary-foreground focus-visible:ring-ring mt-2 px-3 py-1.5 text-sm font-medium focus-visible:ring-1 focus-visible:outline-hidden rounded-pgs"
        >
          Try again
        </button>
      )}
    </div>
  );
}
