// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { useSession } from '@paigasus/auth/client';
import { reasonText } from '@paigasus/discovery/client';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@paigasus/ui';
import { navRows, type NavEntry } from '../nav/rows';
import { useZone } from '../zone/context';
import { ZoneLink } from '../zone/zone-link';
import { BrandLink, type Brand } from './brand-link';
import { BRAND_LOGO_DARK_SRC, BRAND_LOGO_LIGHT_SRC } from './brand-logo';

/*
 * Lucide's `layout-grid` (ISC), inline like OrganizationIcon: the "switch console" glyph the mark
 * turns into. Decorative: the trigger's name says what it does.
 */
function ConsoleSwitchGlyph(): ReactElement {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      className="size-5"
    >
      <rect width="7" height="7" x="3" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="14" rx="1" />
      <rect width="7" height="7" x="3" y="14" rx="1" />
    </svg>
  );
}

export type ConsoleSwitcherProps = { readonly brand: Brand; readonly consoles: readonly NavEntry[] };

/*
 * The brand as a console switcher (SMA-783). On hover, keyboard focus or while open, the Pegasus
 * mark fades and shrinks into a grid glyph, so the brand visibly becomes a control; activating it
 * (click, Enter, Space, ArrowDown) opens the menu of consoles. The menu does NOT open on hover: a
 * pointer passing over the header would pop it open, and touch has no hover at all.
 *
 * `consoles` follows the Primary nav's entry rules (../nav/rows, without rule 2a): an unconfigured
 * zone is left out, an absent console is hidden, a degraded one is a disabled item with its reason.
 * The current console carries a tick and aria-current. With fewer than two consoles left there is
 * nothing to switch to, so this is the plain BrandLink.
 */
export function ConsoleSwitcher({ brand, consoles }: ConsoleSwitcherProps): ReactElement {
  const session = useSession();
  const { zone: currentZone, zones } = useZone();
  const rows = navRows('ConsoleSwitcher', consoles, zones, session);
  if (rows.length < 2) return <BrandLink brand={brand} />;

  const morph = 'transition duration-200 ease-out motion-reduce:transition-none';
  const out = 'group-hover:scale-75 group-hover:opacity-0 group-focus-visible:scale-75 group-focus-visible:opacity-0 group-data-[state=open]:scale-75 group-data-[state=open]:opacity-0';

  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="group -mx-1 flex items-center gap-2 rounded-pgs px-1 py-0.5 font-semibold outline-hidden hover:bg-muted focus-visible:ring-1 focus-visible:ring-ring data-[state=open]:bg-muted">
        <span className="relative size-8 shrink-0">
          <img src={BRAND_LOGO_LIGHT_SRC} alt="" width={32} height={32} className={`absolute inset-0 size-8 dark:hidden ${morph} ${out}`} />
          <img src={BRAND_LOGO_DARK_SRC} alt="" width={32} height={32} className={`absolute inset-0 hidden size-8 dark:block ${morph} ${out}`} />
          <span
            aria-hidden="true"
            className={`absolute inset-0 flex scale-75 items-center justify-center text-muted-foreground opacity-0 group-hover:scale-100 group-hover:opacity-100 group-focus-visible:scale-100 group-focus-visible:opacity-100 group-data-[state=open]:scale-100 group-data-[state=open]:opacity-100 ${morph}`}
          >
            <ConsoleSwitchGlyph />
          </span>
        </span>
        {/* "Switch console:" is screen-reader text; the visible label stays inside the name (WCAG
            2.5.3). The space after it is its own node, for the reason Switcher gives. */}
        <span className="sr-only">Switch console: Paigasus</span> <span>{brand.label}</span>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-56">
        <DropdownMenuLabel className="text-xs text-muted-foreground">Consoles</DropdownMenuLabel>
        <DropdownMenuSeparator />
        {rows.map((row) => {
          const key = `${row.entry.zone}:${row.entry.href}`;
          if (row.kind === 'degraded') {
            return (
              <DropdownMenuItem key={key} disabled className="flex-col items-start gap-0 pl-8 text-muted-foreground data-[disabled]:opacity-100">
                {/* Full opacity, muted text: the menu's default 50% would push the reason below a
                    readable contrast. The same treatment as the Primary nav's degraded entry. */}
                <span>{row.entry.label}</span>
                <span className="text-xs text-muted-foreground">{reasonText(row.service, row.reason)}</span>
              </DropdownMenuItem>
            );
          }
          const isCurrent = row.entry.zone === currentZone;
          return (
            <DropdownMenuItem key={key} asChild>
              <ZoneLink href={row.entry.href} aria-current={isCurrent ? 'true' : undefined}>
                <span aria-hidden="true" className="inline-block w-4">
                  {isCurrent ? '✓' : ''}
                </span>
                {row.entry.label}
              </ZoneLink>
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
