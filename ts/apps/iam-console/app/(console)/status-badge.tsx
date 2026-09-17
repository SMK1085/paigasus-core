// SPDX-License-Identifier: Apache-2.0
//
// The badge next to the `h1` of a detail page (SMA-630 spec § 6.3). An active node gets none.
import type { ReactElement } from 'react';
import { BADGE_LABEL, lifecycleView, type NodeLifecycle } from './node-status';

export function StatusBadge({ lifecycle }: { readonly lifecycle: NodeLifecycle }): ReactElement | null {
  const label = BADGE_LABEL[lifecycleView(lifecycle)];
  if (label === null) return null;
  return (
    <span data-testid="node-status" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
      {label}
    </span>
  );
}
