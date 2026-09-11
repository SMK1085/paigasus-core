// SPDX-License-Identifier: Apache-2.0
'use client';

import { usePathname } from 'next/navigation';
import { useId, type ReactElement } from 'react';
import { can, useSession, type SessionView } from '@paigasus/auth/client';
import { reasonText } from '@paigasus/discovery/client';
import type { DegradedReason } from '@paigasus/discovery/types';
import { cn } from '@paigasus/ui';
import { useZone } from '../zone/context';
import { ZoneLinkError } from '../zone/errors';
import { isAtOrUnder, pathOf, resolveZone } from '../zone/resolve';
import { ZoneLink } from '../zone/zone-link';
import type { NavEntryState } from './state';

export type NavEntry = {
  /** The zone the entry belongs to. Rule 1 uses it, so AC 3 holds with a root zone (spec § 7.2). */
  readonly zone: string;
  /** The full path (spec § 6.2). It must resolve to `zone`. */
  readonly href: string;
  readonly label: string;
  /** From navStateOf(), resolved on the server. */
  readonly state: NavEntryState;
  /** A cosmetic gate through can() (F7). `/client` exports no RoleGrantRef, hence the indexed type. */
  readonly requires?: SessionView['grants'][number];
};

export type PrimaryNavProps = { readonly entries: readonly NavEntry[] };

type Row =
  | { readonly kind: 'link'; readonly entry: NavEntry; readonly path: string; readonly sameZone: boolean }
  | { readonly kind: 'degraded'; readonly entry: NavEntry; readonly service: string; readonly reason: DegradedReason };

/**
 * Of the same-zone link rows whose path matches the pathname, the one with the longest path
 * (§ 7.4). The candidate test is `isAtOrUnder` from ../zone/resolve: the same segment-boundary
 * rule `resolveZone` uses, so the two cannot drift apart.
 */
function activeEntry(rows: readonly Row[], pathname: string): NavEntry | null {
  let best: { readonly entry: NavEntry; readonly length: number } | null = null;
  for (const row of rows) {
    if (row.kind !== 'link' || !row.sameZone || !isAtOrUnder(pathname, row.path)) continue;
    if (best === null || row.path.length > best.length) best = { entry: row.entry, length: row.path.length };
  }
  return best?.entry ?? null;
}

/**
 * The primary navigation. For each entry, the FIRST rule that applies decides (spec § 7.2):
 *   1. entry.zone is not an own key of the zone map        -> nothing
 *   2. entry.href does not resolve to entry.zone           -> ZoneLinkError
 *   3. state 'absent'                                      -> nothing
 *   4. `requires` is set and can() is false                -> nothing (cosmetic only)
 *   5. state 'degraded'                                    -> the disabled entry, with a visible reason
 *   6. state 'available'                                   -> a ZoneLink, aria-current on the active one
 * Returns null when no entry remains: no empty nav landmark.
 *
 * useSession() is called unconditionally (rules of hooks). PrimaryNav is rendered only inside
 * AppShell, which is always under a SessionProvider.
 */
export function PrimaryNav({ entries }: PrimaryNavProps): ReactElement | null {
  const session = useSession();
  const { zone: currentZone, zones } = useZone();
  const pathname = usePathname();
  const baseId = useId();

  const rows: Row[] = [];
  for (const entry of entries) {
    if (!Object.hasOwn(zones, entry.zone)) continue;
    const target = resolveZone(entry.href, zones);
    if (target === null || target.zone !== entry.zone) {
      throw new ZoneLinkError(`PrimaryNav: an entry declared for zone "${entry.zone}" has an href that does not resolve to that zone. (The href is withheld.)`);
    }
    if (entry.state.state === 'absent') continue;
    if (entry.requires !== undefined && !can(session, entry.requires)) continue;
    if (entry.state.state === 'degraded') {
      rows.push({ kind: 'degraded', entry, service: entry.state.service, reason: entry.state.reason });
      continue;
    }
    rows.push({ kind: 'link', entry, path: pathOf(target.rest), sameZone: target.zone === currentZone });
  }
  if (rows.length === 0) return null;

  const active = activeEntry(rows, pathname);

  return (
    <nav aria-label="Primary">
      <ul className="flex items-center gap-4">
        {rows.map((row, index) => {
          const key = `${row.entry.zone}:${row.entry.href}`;
          if (row.kind === 'degraded') {
            const reasonId = `${baseId}-reason-${String(index)}`;
            return (
              <li key={key} className="flex flex-col">
                {/* No href and no <a>: nothing can navigate, before or after hydration (spec § 7.3).
                    role="link" + aria-disabled is the WAI-ARIA disabled-link pattern; tabIndex 0 keeps
                    it reachable, and there is no key handler, so Enter does nothing. */}
                <span role="link" aria-disabled="true" tabIndex={0} aria-describedby={reasonId} className="cursor-not-allowed text-sm text-muted-foreground">
                  {row.entry.label}
                </span>
                {/* A SIBLING, not a child: as a child it would be part of the name AND the description. */}
                <span id={reasonId} className="text-xs text-muted-foreground">
                  {reasonText(row.service, row.reason)}
                </span>
              </li>
            );
          }
          const current = row.entry === active;
          return (
            <li key={key}>
              <ZoneLink href={row.entry.href} aria-current={current ? 'page' : undefined} className={cn('text-sm hover:underline', current && 'font-semibold')}>
                {row.entry.label}
              </ZoneLink>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
