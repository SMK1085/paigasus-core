// SPDX-License-Identifier: Apache-2.0
import type { ReactElement, ReactNode } from 'react';
import { SessionProvider, type SessionView } from '@paigasus/auth/client';
import { ZoneProvider } from '../../src/zone/context';
import type { ZoneMap } from '../../src/zone/resolve';

export const ZONES: ZoneMap = { iam: '/iam', gateway: '/gateway' };

/** grantsAvailable is TRUE here, so `can()` really decides. A test that needs fail-open overrides it. */
export const SESSION: SessionView = {
  principalPrn: 'prn:paigasus:iam::user/0190f5b4-0000-7000-8000-000000000001',
  displayName: 'Ada Lovelace',
  email: 'ada@example.test',
  grants: [],
  grantsAvailable: true,
};

export type ProviderOptions = { readonly zone?: string; readonly zones?: ZoneMap; readonly session?: SessionView };

/** `ui` under a ZoneProvider only (the zone defaults to iam and the map to ZONES). */
export function inZone(ui: ReactNode, options: ProviderOptions = {}): ReactElement {
  return (
    <ZoneProvider zone={options.zone ?? 'iam'} zones={options.zones ?? ZONES}>
      {ui}
    </ZoneProvider>
  );
}

/** `ui` under a ZoneProvider AND a SessionProvider — what AppShell needs (spec § 8.1). */
export function inShell(ui: ReactNode, options: ProviderOptions = {}): ReactElement {
  return inZone(<SessionProvider value={options.session ?? SESSION}>{ui}</SessionProvider>, options);
}
