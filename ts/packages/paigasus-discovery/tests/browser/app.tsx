// SPDX-License-Identifier: Apache-2.0
//
// Shared between fixture-server.ts (which renders this to real HTML SERVER-SIDE, giving the
// JS-disabled spec genuine markup to click) and entry.tsx (which HYDRATES the exact same tree
// client-side). One component for both keeps "what got rendered" and "what React attaches to"
// from drifting into two hand-written copies of the same tree.
//
// The child carries its OWN `href`, distinct from anything the ancestor does. The pre-hydration
// spec's only reliable, script-free signal is whether clicking navigates to THIS href — a
// script-driven "ancestor" flag cannot fire at all before hydration, regardless of the CSS under
// test — see capability-hit-test.spec.ts's header for the full reasoning.
import type { ReactElement } from 'react';
import { CapabilityDisabled } from '../../src/disabled.js';

export const CHILD_HREF = '/audit-target';

export function markAncestorClicked(): void {
  (window as Window & { __ancestorClicked?: boolean }).__ancestorClicked = true;
}

export function App(): ReactElement {
  return (
    <div
      data-testid="ancestor"
      role="button"
      tabIndex={0}
      onClick={markAncestorClicked}
      onKeyDown={(event) => {
        if (event.key === 'Enter' || event.key === ' ') markAncestorClicked();
      }}
    >
      <p>A large clickable row — the composition the design change exists for.</p>
      <CapabilityDisabled service="iam" reason="network">
        <a href={CHILD_HREF} data-testid="child-link">
          Audit
        </a>
      </CapabilityDisabled>
    </div>
  );
}
