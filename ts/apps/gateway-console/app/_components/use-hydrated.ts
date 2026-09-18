// SPDX-License-Identifier: Apache-2.0
//
// true after hydration; false in the server render and while React hydrates (SMA-636 spec § 5.4
// rule 7, plan SPEC DEVIATION 7). useSyncExternalStore gives the server snapshot during hydration
// and the client snapshot after it, with no effect and no state. A control that needs JavaScript
// renders only when this is true, so with JavaScript off it does not exist, and no document-level
// POST can run it.
'use client';

import { useSyncExternalStore } from 'react';

const subscribe = (): (() => void) => () => undefined;

export function useHydrated(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => true,
    () => false,
  );
}
