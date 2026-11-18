// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement, ReactNode } from 'react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@paigasus/ui';
import { useZone } from '../zone/context';
import { resolveZone } from '../zone/resolve';
import { ZoneLink } from '../zone/zone-link';
import { initialsOf } from './initials';

export type SwitcherItem = { readonly id: string; readonly label: string; readonly href: string };

export type SwitcherProps = {
  /** "Organisation", "Team". One instance holds one level; an app with orgs and teams renders two. */
  readonly label: string;
  readonly items: readonly SwitcherItem[];
  /** The selected item's id. The URL owns the selection (D1), so the app derives it from the route. */
  readonly currentId: string | null;
  /** A muted 16px icon shown in place of the "Label:" prefix; the label stays as screen-reader text. */
  readonly icon?: ReactNode;
  /**
   * A square badge with the current item's initials in place of the prefix (SMA-783), taking
   * precedence over `icon`, which still shows while nothing is selected. With it the current
   * item's name is truncated past 12rem and hidden below the `md` breakpoint, where the badge
   * alone stands for it; the full name stays in the accessible name and the `title` tooltip.
   */
  readonly initials?: boolean;
};

/**
 * An org or team switcher on @paigasus/ui's DropdownMenu. Radix owns focus management, arrow keys,
 * typeahead, Escape and focus return (ADR-0021 decision 5 forbids a hand-rolled menu).
 * Choosing an item is a navigation.
 */
export function Switcher({ label, items, currentId, icon, initials = false }: SwitcherProps): ReactElement | null {
  const { zones } = useZone();
  // FILTER FIRST (F18). A DropdownMenuItem whose asChild child renders null stays registered with
  // Radix, and a keyboard open then calls .focus() on null and throws. So an unmatched item is
  // dropped BEFORE any DropdownMenuItem is rendered, never left to ZoneLink's null.
  const remaining = items.filter((item) => resolveZone(item.href, zones) !== null);
  if (remaining.length === 0) return null;

  const current = remaining.find((item) => item.id === currentId) ?? null;
  const currentLabel = current?.label ?? 'none selected';
  const letters = initials && current !== null ? initialsOf(current.label, null) : null;
  const compact = icon !== undefined || initials;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger title={compact ? `${label}: ${currentLabel}` : undefined} className="inline-flex min-w-0 items-center gap-1.5 rounded-pgs border border-border px-2 py-1 text-sm">
        {compact ? (
          <>
            {letters !== null ? (
              // Square with radius-xs, so it never reads as the round user avatar beside it.
              <span aria-hidden="true" className="inline-flex size-5 shrink-0 items-center justify-center rounded-xs bg-muted text-xs leading-none font-semibold tracking-tight text-foreground">
                {letters}
              </span>
            ) : icon !== undefined ? (
              <span aria-hidden="true" className="inline-flex size-4 shrink-0 text-muted-foreground [&>svg]:size-4">
                {icon}
              </span>
            ) : null}
            <span className="sr-only">{`${label}:`}</span>
            {/* The accessible name joins adjacent inline text with no separator and drops a trailing
                space inside the sr-only span, so without this space it reads "Organisation:Acme"
                (the same reason as BrandLink's). */}{' '}
            <span className={letters !== null ? 'max-w-48 truncate max-md:sr-only' : undefined}>{currentLabel}</span>
          </>
        ) : (
          `${label}: ${currentLabel}`
        )}
      </DropdownMenuTrigger>
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
