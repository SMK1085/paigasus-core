// SPDX-License-Identifier: Apache-2.0
'use client';

import type { ReactElement } from 'react';
import { ZoneLink } from '../zone/zone-link';
import { BRAND_LOGO_DARK_SRC, BRAND_LOGO_LIGHT_SRC } from './brand-logo';

export type Brand = {
  /** The product name that follows the logo, for example "IAM". The logo stands for "Paigasus". */
  readonly label: string;
  /** A full path (spec § 6.2). */
  readonly href: string;
};

/**
 * The header's home link: the Paigasus logo, then the product label.
 *
 * The logo has a light-theme and a dark-theme variant, and the `dark:` variant shows one and hides
 * the other. Both images are decorative (alt=""): a hidden image leaves the accessibility tree, so
 * alt text on them would make the link's name depend on the theme. The visually hidden "Paigasus"
 * gives the link the same accessible name in both themes, for example "Paigasus IAM".
 *
 * Plain <img>, not next/image: the sources are small data URIs, so the image optimizer has nothing
 * to do, and this package must also render outside a Next app (vitest, the e2e fixture).
 */
export function BrandLink({ brand }: { readonly brand: Brand }): ReactElement {
  return (
    <ZoneLink href={brand.href} className="flex items-center gap-2 font-semibold">
      <img src={BRAND_LOGO_LIGHT_SRC} alt="" width={32} height={32} className="size-8 dark:hidden" />
      <img src={BRAND_LOGO_DARK_SRC} alt="" width={32} height={32} className="hidden size-8 dark:block" />
      <span className="sr-only">Paigasus</span>
      {/* The accessible name joins adjacent inline text with no separator, so without this space
          the link reads "PaigasusIAM". A flex container does not render a space-only text node. */}{' '}
      <span>{brand.label}</span>
    </ZoneLink>
  );
}
