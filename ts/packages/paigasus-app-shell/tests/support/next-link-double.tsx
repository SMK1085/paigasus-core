// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/link` in the jsdom tier (spec § 10.2). It renders <a data-next-link>, so a
// test can tell a soft next/link from a plain <a>. It records every click's href and cancels the
// click, so jsdom never attempts a navigation. A test file installs it with:
//   vi.mock('next/link', () => import('../support/next-link-double'));
import type { AnchorHTMLAttributes, ReactElement, Ref } from 'react';

/** Every href a click on the double activated, in order. Reset it in `beforeEach`. */
export const nextLinkClicks: string[] = [];

type NextLinkDoubleProps = Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href'> & { href: string; ref?: Ref<HTMLAnchorElement> };

export default function NextLinkDouble({ href, onClick, children, ...rest }: NextLinkDoubleProps): ReactElement {
  return (
    <a
      {...rest}
      data-next-link=""
      href={href}
      onClick={(event) => {
        onClick?.(event);
        nextLinkClicks.push(href);
        event.preventDefault();
      }}
    >
      {children}
    </a>
  );
}
