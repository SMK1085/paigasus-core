// SPDX-License-Identifier: Apache-2.0
'use client';

import { useEffect, type ReactElement, type ReactNode } from 'react';
import { SessionProvider, type SessionView } from '@paigasus/auth/client';
// The package imports ITSELF by name (Node package self-reference, measured in the spike). This is
// what keeps ONE instance of @paigasus/ui — and so one LinkContext — in the client bundle. E4's
// same-zone row fails if there are two.
import { ZoneLink, ZoneProvider } from '@paigasus/app-shell';
import { LinkProvider } from '@paigasus/ui';

// A LITERAL map (spec § 10.3). The fixture must not import @paigasus/next-config/runtime; the
// boundary rule bans it here. Module scope keeps the identity stable across renders.
const ZONES = { iam: '/iam', gateway: '/gateway' };

const SESSION: SessionView = {
  principalPrn: null,
  displayName: 'Ada Lovelace',
  email: 'ada@example.test',
  grants: [],
  grantsAvailable: false,
};

export function Providers({ children }: { children: ReactNode }): ReactElement {
  useEffect(() => {
    // The hydration signal every spec waits for. A click before hydration is a document navigation
    // for ANY link, so E1 would fail for the wrong reason without it.
    document.documentElement.dataset.hydrated = 'true';
  }, []);
  return (
    <ZoneProvider zone="iam" zones={ZONES}>
      <SessionProvider value={SESSION}>
        <LinkProvider link={ZoneLink}>{children}</LinkProvider>
      </SessionProvider>
    </ZoneProvider>
  );
}
