// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { useZone } from '../zone/context';
import { resolveZone } from '../zone/resolve';
import { ZoneLink } from '../zone/zone-link';

export type Crumb = {
  readonly label: string;
  /** A full path (spec § 6.2). The last crumb never links, even with an href. */
  readonly href?: string;
};

export type BreadcrumbsProps = { readonly items: readonly Crumb[] };

/**
 * A page's trail. A page renders it itself: a root layout renders the shell once for many pages,
 * and only a page knows its own trail (spec § 8.1).
 *
 * A crumb into another zone is a ZoneLink, so it is a hard navigation. A crumb whose href matches
 * no zone is plain text, and the trail keeps its length. Like the other list components, it checks
 * resolveZone itself instead of relying on ZoneLink's null.
 */
export function Breadcrumbs({ items }: BreadcrumbsProps): ReactElement | null {
  const { zones } = useZone();
  if (items.length === 0) return null;

  return (
    <nav aria-label="Breadcrumb">
      <ol className="flex flex-wrap items-center gap-1 text-sm">
        {items.map((item, index) => {
          const last = index === items.length - 1;
          const key = `${item.label}|${item.href ?? ''}`;
          let content: ReactElement;
          if (last) {
            // The last crumb never links, but a malformed href must still throw like every other
            // href in the package (F8). The result is not used: only the validation matters here.
            if (item.href !== undefined) resolveZone(item.href, zones);
            content = <span aria-current="page">{item.label}</span>;
          } else if (item.href !== undefined && resolveZone(item.href, zones) !== null) {
            content = (
              <ZoneLink href={item.href} className="hover:underline">
                {item.label}
              </ZoneLink>
            );
          } else {
            content = <span>{item.label}</span>;
          }
          return (
            <li key={key} className="flex items-center gap-1">
              {index > 0 ? (
                <span aria-hidden="true" className="text-muted-foreground">
                  /
                </span>
              ) : null}
              {content}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
