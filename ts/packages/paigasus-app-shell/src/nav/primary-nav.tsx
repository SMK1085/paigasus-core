// SPDX-License-Identifier: Apache-2.0
'use client';

import { usePathname } from 'next/navigation';
import { useId, type ReactElement } from 'react';
import { useSession } from '@paigasus/auth/client';
import { reasonText } from '@paigasus/discovery/client';
import { cn } from '@paigasus/ui';
import { useZone } from '../zone/context';
import { isAtOrUnder, pathOf } from '../zone/resolve';
import { ZoneLink } from '../zone/zone-link';
import { navRows, type NavEntry, type NavRow } from './rows';

export type { NavEntry };

export type PrimaryNavProps = { readonly entries: readonly NavEntry[] };

/**
 * Of the link rows whose path matches the pathname, the one with the longest path (§ 7.4). The
 * candidate test is `isAtOrUnder` from ../zone/resolve: the same segment-boundary rule
 * `resolveZone` uses, so the two cannot drift apart. Every row is in the current zone (rule 2a).
 */
function activeEntry(rows: readonly NavRow[], pathname: string): NavEntry | null {
  let best: { readonly entry: NavEntry; readonly length: number } | null = null;
  for (const row of rows) {
    if (row.kind !== 'link') continue;
    const path = pathOf(row.target.rest);
    if (!isAtOrUnder(pathname, path)) continue;
    if (best === null || path.length > best.length) best = { entry: row.entry, length: path.length };
  }
  return best?.entry ?? null;
}

type Section = { readonly label: string | undefined; readonly rows: NavRow[] };

/** Rows gathered under their group, groups in the order of their first row (SMA-783). */
function sectionsOf(rows: readonly NavRow[]): Section[] {
  const sections: Section[] = [];
  for (const row of rows) {
    const label = row.entry.group;
    const section = sections.find((candidate) => candidate.label === label);
    if (section === undefined) sections.push({ label, rows: [row] });
    else section.rows.push(row);
  }
  return sections;
}

/**
 * The primary navigation of the CURRENT console, as a left sidebar (SMA-783). The entry rules are
 * navRows()'s (./rows), with rule 2a on: an entry for another configured zone throws, because
 * another console is reached through the console switcher, never through this list.
 * Returns null when no entry remains: no empty nav landmark, and so no empty sidebar column.
 *
 * useSession() is called unconditionally (rules of hooks). PrimaryNav is rendered only inside
 * AppShell, which is always under a SessionProvider.
 */
export function PrimaryNav({ entries }: PrimaryNavProps): ReactElement | null {
  const session = useSession();
  const { zone: currentZone, zones } = useZone();
  const pathname = usePathname();
  const baseId = useId();

  const rows = navRows('PrimaryNav', entries, zones, session, currentZone);
  if (rows.length === 0) return null;

  const active = activeEntry(rows, pathname);

  return (
    <nav aria-label="Primary" className="w-56 shrink-0 border-r border-border p-2">
      <div className="flex flex-col gap-4">
        {sectionsOf(rows).map((section, sectionIndex) => {
          const headingId = `${baseId}-group-${String(sectionIndex)}`;
          return (
            <div key={section.label ?? `untitled-${String(sectionIndex)}`} className="flex flex-col gap-1">
              {section.label === undefined ? null : (
                <span id={headingId} className="px-2 text-xs font-medium text-muted-foreground">
                  {section.label}
                </span>
              )}
              <ul className="flex flex-col gap-0.5" aria-labelledby={section.label === undefined ? undefined : headingId}>
                {section.rows.map((row) => {
                  const key = `${row.entry.zone}:${row.entry.href}`;
                  if (row.kind === 'degraded') {
                    const reasonId = `${baseId}-reason-${String(rows.indexOf(row))}`;
                    return (
                      <li key={key} className="flex flex-col px-2 py-1.5">
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
                      <ZoneLink
                        href={row.entry.href}
                        aria-current={current ? 'page' : undefined}
                        className={cn('flex rounded-pgs px-2 py-1.5 text-sm text-muted-foreground hover:bg-muted hover:text-foreground', current && 'bg-muted font-medium text-foreground')}
                      >
                        {row.entry.label}
                      </ZoneLink>
                    </li>
                  );
                })}
              </ul>
            </div>
          );
        })}
      </div>
    </nav>
  );
}
