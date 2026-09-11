// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@paigasus/ui';
import { useZone } from '../zone/context';
import { resolveZone } from '../zone/resolve';
import { ZoneLink } from '../zone/zone-link';

export type SwitcherItem = { readonly id: string; readonly label: string; readonly href: string };

export type SwitcherProps = {
  /** "Organisation", "Team". One instance holds one level; an app with orgs and teams renders two. */
  readonly label: string;
  readonly items: readonly SwitcherItem[];
  /** The selected item's id. The URL owns the selection (D1), so the app derives it from the route. */
  readonly currentId: string | null;
};

/**
 * An org or team switcher on @paigasus/ui's DropdownMenu. Radix owns focus management, arrow keys,
 * typeahead, Escape and focus return (ADR-0021 decision 5 forbids a hand-rolled menu).
 * Choosing an item is a navigation.
 */
export function Switcher({ label, items, currentId }: SwitcherProps): ReactElement | null {
  const { zones } = useZone();
  // FILTER FIRST (F18). A DropdownMenuItem whose asChild child renders null stays registered with
  // Radix, and a keyboard open then calls .focus() on null and throws. So an unmatched item is
  // dropped BEFORE any DropdownMenuItem is rendered, never left to ZoneLink's null.
  const remaining = items.filter((item) => resolveZone(item.href, zones) !== null);
  if (remaining.length === 0) return null;

  const current = remaining.find((item) => item.id === currentId) ?? null;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="rounded-pgs border border-border px-2 py-1 text-sm">{`${label}: ${current?.label ?? 'none selected'}`}</DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {remaining.map((item) => {
          const isCurrent = item.id === current?.id;
          return (
            <DropdownMenuItem key={item.id} asChild>
              <ZoneLink href={item.href} aria-current={isCurrent ? 'true' : undefined}>
                <span aria-hidden="true" className="inline-block w-4">
                  {isCurrent ? '✓' : ''}
                </span>
                {item.label}
              </ZoneLink>
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
