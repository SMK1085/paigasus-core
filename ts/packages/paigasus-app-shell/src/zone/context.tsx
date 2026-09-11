// SPDX-License-Identifier: Apache-2.0
'use client';

import { createContext, use, useMemo, type ReactElement, type ReactNode } from 'react';
import { ZoneProviderMissingError } from './errors';
import { assertZoneMap, type ZoneMap } from './resolve';

export type ZoneContextValue = {
  readonly zone: string;
  /** The current zone's canonical base path (`''` for a root-mounted zone). */
  readonly basePath: string;
  readonly zones: ZoneMap;
};

export type ZoneProviderProps = {
  /** This app's zone id. It must be an OWN key of `zones`. */
  readonly zone: string;
  /** Zone id -> canonical base path, from getPublicConfig(). Nothing else crosses to the browser. */
  readonly zones: ZoneMap;
  readonly children: ReactNode;
};

// Undefined by default, on purpose: useZone() throws instead of falling back (spec § 6.1).
const ZoneContext = createContext<ZoneContextValue | undefined>(undefined);

/**
 * Hands the zone map to ZoneLink and the shell. The app's server layout calls getPublicConfig()
 * and passes `zone` and `zones` as props (spec § 6.1). Throws ZoneConfigError during render when
 * `zone` is not an own key of `zones`, or when two zones share a base path.
 */
export function ZoneProvider({ zone, zones, children }: ZoneProviderProps): ReactElement {
  const value = useMemo<ZoneContextValue>(() => ({ zone, basePath: assertZoneMap(zone, zones), zones }), [zone, zones]);
  return <ZoneContext value={value}>{children}</ZoneContext>;
}

export function useZone(): ZoneContextValue {
  const value = use(ZoneContext);
  if (value === undefined) throw new ZoneProviderMissingError();
  return value;
}
