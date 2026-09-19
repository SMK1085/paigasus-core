// SPDX-License-Identifier: Apache-2.0
//
// Offset paging links (SMA-636 D15). It follows iam-console's app/_components/pager.tsx with two
// changes: every link has prefetch={false} (spec § 4.1), and `keep` may carry the selected account
// id as well as the other list's offset. No 'use client' and no server-only: the client section
// renders it, and ZoneLink is a client component. Hrefs are full paths (/gateway/…).
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { PAGE_SIZE, linkHref } from '../../lib/paging';

export type PagerProps = {
  readonly label: string;
  readonly path: string;
  readonly param: string;
  readonly offset: number;
  readonly nextOffset: number | null;
  /** The other query parameters of the page, kept in every link. */
  readonly keep?: Readonly<Record<string, string | number | null>>;
};

export function Pager({ label, path, param, offset, nextOffset, keep = {} }: PagerProps): ReactElement | null {
  if (offset === 0 && nextOffset === null) return null;
  const previous = Math.max(0, offset - PAGE_SIZE);
  return (
    <nav aria-label={label} className="flex gap-4 text-sm">
      {offset > 0 ? (
        <ZoneLink prefetch={false} href={linkHref(path, { ...keep, [param]: previous })} className="hover:underline">
          Previous
        </ZoneLink>
      ) : null}
      {nextOffset === null ? null : (
        <ZoneLink prefetch={false} href={linkHref(path, { ...keep, [param]: nextOffset })} className="hover:underline">
          Next
        </ZoneLink>
      )}
    </nav>
  );
}
