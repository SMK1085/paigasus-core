// SPDX-License-Identifier: Apache-2.0
'use client';

import NextLink from 'next/link';
import { useEffect, type AnchorHTMLAttributes, type ReactElement, type Ref } from 'react';
import type { LinkProps } from '@paigasus/ui';
import { useZone } from './context';
import { pathOf, resolveZone } from './resolve';

/**
 * A superset of @paigasus/ui's LinkProps, so ZoneLink is assignable to LinkComponent and an app can
 * inject it: <LinkProvider link={ZoneLink}> (spec § 6.4). There is no `prefetch`, `replace` or
 * `scroll`: Next's defaults apply, and nothing consumes them yet.
 */
export type ZoneLinkProps = LinkProps & Omit<AnchorHTMLAttributes<HTMLAnchorElement>, 'href' | 'onMouseEnter' | 'onTouchStart' | 'onClick'> & { ref?: Ref<HTMLAnchorElement> };

// One development warning per first path segment, for the life of the page.
const warnedSegments = new Set<string>();

/** Login and logout are route handlers, not pages. A soft link to them starts an RSC fetch (§ 6.5). */
function isAuthRoute(rest: string): boolean {
  const path = pathOf(rest);
  return path === '/auth' || path.startsWith('/auth/');
}

/**
 * A link that knows the zone map (ADR-0017). The href is the FULL path, as the ingress sees it:
 * "/gateway/usage", "/iam/users?page=2".
 *
 *   no zone matches          -> nothing (plus one development warning)
 *   same zone, under /auth/  -> a plain <a href={href}>
 *   same zone                -> <NextLink href={rest}>; Next adds the base path back (F3)
 *   another zone             -> a plain <a href={href}>, a hard navigation
 *
 * Throws ZoneLinkError for a malformed href (spec § 6.2). It forwards `ref` and every anchor
 * attribute, so the props that Radix `asChild` passes reach the DOM.
 */
export function ZoneLink({ href, children, ...anchorProps }: ZoneLinkProps): ReactElement | null {
  const { zone, zones } = useZone();
  const target = resolveZone(href, zones);
  const unmatched = target === null;

  useEffect(() => {
    if (!unmatched || process.env.NODE_ENV === 'production') return;
    // The first segment only. The full href can carry user data.
    const segment = pathOf(href).split('/')[1] ?? '';
    if (warnedSegments.has(segment)) return;
    warnedSegments.add(segment);
    console.warn(
      `ZoneLink: no configured zone matches the path segment "/${segment}", so the link renders nothing. ` +
        `Write the full path as the ingress sees it (for example "/<zone>/${segment}"). A zone-relative href matches no zone.`,
    );
  }, [unmatched, href]);

  if (target === null) return null;
  if (target.zone === zone && !isAuthRoute(target.rest)) {
    return (
      <NextLink {...anchorProps} href={target.rest}>
        {children}
      </NextLink>
    );
  }
  return (
    <a {...anchorProps} href={href}>
      {children}
    </a>
  );
}
