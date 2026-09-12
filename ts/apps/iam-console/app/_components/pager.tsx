// SPDX-License-Identifier: Apache-2.0
//
// Offset paging links (spec § 5.2). A SERVER component: ZoneLink is a client component, and a
// server component may render it with string props. Hrefs are full paths (/iam/…), as ZoneLink needs.
// On a page with two lists, `keep` carries the other list's offset, so a link keeps that list's page.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { PAGE_SIZE, pageHref } from '../../lib/paging';

export type PagerProps = {
  readonly label: string;
  readonly path: string;
  readonly param: string;
  readonly offset: number;
  readonly nextOffset: number | null;
  /** The offsets of the other lists on the page, by query parameter. */
  readonly keep?: Readonly<Record<string, number>>;
};

export function Pager({ label, path, param, offset, nextOffset, keep }: PagerProps): ReactElement | null {
  if (offset === 0 && nextOffset === null) return null;
  const previous = Math.max(0, offset - PAGE_SIZE);
  return (
    <nav aria-label={label} className="flex gap-4 text-sm">
      {offset > 0 ? (
        <ZoneLink href={pageHref(path, param, previous, keep)} className="hover:underline">
          Previous
        </ZoneLink>
      ) : null}
      {nextOffset === null ? null : (
        <ZoneLink href={pageHref(path, param, nextOffset, keep)} className="hover:underline">
          Next
        </ZoneLink>
      )}
    </nav>
  );
}
