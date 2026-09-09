// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement, ReactNode } from 'react';

/*
 * Hand-written, not shadcn-generated — see dialog.tsx and the package README for why. Plain
 * composition, no radix primitive needed: a heading, optional prose, and an optional action
 * slot the caller fills (with `Link` from `../nav/link` for a navigational action, or a
 * `Button`-shaped element for an imperative one — this component does not care which).
 */

export interface EmptyStateProps {
  title: string;
  description?: string;
  action?: ReactNode;
}

export function EmptyState({ title, description, action }: EmptyStateProps): ReactElement {
  return (
    <div className="border-border flex flex-col items-center gap-2 border border-dashed p-8 text-center rounded-pgs">
      <h2 className="text-foreground text-sm font-medium">{title}</h2>
      {description === undefined ? null : <p className="text-muted-foreground text-sm">{description}</p>}
      {action === undefined ? null : <div className="mt-2">{action}</div>}
    </div>
  );
}
