// SPDX-License-Identifier: Apache-2.0
import { clsx, type ClassValue } from 'clsx';
import { twMerge } from 'tailwind-merge';

/**
 * Join class names, letting a later Tailwind utility win over an earlier conflicting one.
 *
 * This uses plain `tailwind-merge`, which knows Tailwind's own scale names. The token layer
 * in src/styles/tokens.css maps onto those names deliberately, so no configuration is needed.
 * If a genuinely NEW scale is ever added, `extendTailwindMerge` becomes necessary — that is
 * the trigger to revisit this file (SMA-503 spec §12).
 */
export function cn(...inputs: ClassValue[]): string {
  return twMerge(clsx(inputs));
}
