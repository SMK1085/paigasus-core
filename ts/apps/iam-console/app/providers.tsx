// SPDX-License-Identifier: Apache-2.0
'use client';

import { useEffect, type ReactElement, type ReactNode } from 'react';
import { SessionProvider, type SessionView } from '@paigasus/auth/client';
import { ZoneLink, ZoneProvider, type ZoneMap } from '@paigasus/app-shell';
import { LinkProvider } from '@paigasus/ui';

/*
 * The client-side context for every page: the zone map, the session view (console pages only), and
 * the Link implementation @paigasus/ui renders (ADR-0021 decision 3).
 *
 * A client boundary because ZoneLink is a function and cannot cross from a server layout as a prop.
 * `session` is `toSessionView()` output — never a token (ADR-0017). It is null on the public page,
 * which renders PublicShell and needs no SessionProvider.
 */
export function Providers({ zone, zones, session, children }: { zone: string; zones: ZoneMap; session: SessionView | null; children: ReactNode }): ReactElement {
  useEffect(() => {
    // The hydration signal the e2e tier waits for before a click (the @paigasus/app-shell fixture's
    // pattern): a click before hydration is a document navigation for every link.
    document.documentElement.dataset['hydrated'] = 'true';
  }, []);
  const linked = <LinkProvider link={ZoneLink}>{children}</LinkProvider>;
  return (
    <ZoneProvider zone={zone} zones={zones}>
      {session === null ? linked : <SessionProvider value={session}>{linked}</SessionProvider>}
    </ZoneProvider>
  );
}
