// SPDX-License-Identifier: Apache-2.0
//
// The entry rules PrimaryNav and the console switcher share (SMA-510 spec § 7.2, SMA-783). A plain
// helper, not a component: it carries no 'use client' (tests/structure/source-shape.test.ts), and
// both callers are client components, so `can` is a real function wherever this runs.
import { can, type SessionView } from '@paigasus/auth/client';
import type { DegradedReason } from '@paigasus/discovery/types';
import { ZoneLinkError } from '../zone/errors';
import { resolveZone, type ZoneMap, type ZoneTarget } from '../zone/resolve';
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
  /**
   * PrimaryNav only: the heading of the group this entry sits under (SMA-783). Groups appear in the
   * order of their first entry; an entry with no group sits in an untitled group. Ignored by the
   * console switcher.
   */
  readonly group?: string;
};

export type NavRow =
  { readonly kind: 'link'; readonly entry: NavEntry; readonly target: ZoneTarget } | { readonly kind: 'degraded'; readonly entry: NavEntry; readonly service: string; readonly reason: DegradedReason };

/**
 * For each entry, the FIRST rule that applies decides (spec § 7.2):
 *   1. entry.zone is not an own key of the zone map        -> nothing
 *   2. entry.href does not resolve to entry.zone           -> ZoneLinkError
 *   2a. `onlyZone` is set and entry.zone is another zone   -> ZoneLinkError (SMA-783)
 *   3. state 'absent'                                      -> nothing
 *   4. `requires` is set and can() is false                -> nothing (cosmetic only)
 *   5. state 'degraded'                                    -> a degraded row, with its reason
 *   6. state 'available'                                   -> a link row
 * `caller` names the component in an error message. Hrefs are never put in a message.
 */
export function navRows(caller: string, entries: readonly NavEntry[], zones: ZoneMap, session: SessionView, onlyZone?: string): NavRow[] {
  const rows: NavRow[] = [];
  for (const entry of entries) {
    if (!Object.hasOwn(zones, entry.zone)) continue;
    const target = resolveZone(entry.href, zones);
    if (target === null || target.zone !== entry.zone) {
      throw new ZoneLinkError(`${caller}: an entry declared for zone "${entry.zone}" has an href that does not resolve to that zone. (The href is withheld.)`);
    }
    if (onlyZone !== undefined && entry.zone !== onlyZone) {
      throw new ZoneLinkError(
        `${caller}: an entry for zone "${entry.zone}" is not in the current console ("${onlyZone}"). The Primary nav lists the current console's pages only; another console belongs in AppShell's \`consoles\`.`,
      );
    }
    if (entry.state.state === 'absent') continue;
    if (entry.requires !== undefined && !can(session, entry.requires)) continue;
    if (entry.state.state === 'degraded') {
      rows.push({ kind: 'degraded', entry, service: entry.state.service, reason: entry.state.reason });
      continue;
    }
    rows.push({ kind: 'link', entry, target });
  }
  return rows;
}
