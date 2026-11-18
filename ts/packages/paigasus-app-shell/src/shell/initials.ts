// SPDX-License-Identifier: Apache-2.0
//
// The user menu's avatar letters (SMA-783). A plain helper: no 'use client'
// (tests/structure/source-shape.test.ts).

const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });

/** The first user-perceived character: "É" whether precomposed or E + combining accent, "👩🏽" whole. */
function firstGrapheme(text: string): string {
  const first = segmenter.segment(text)[Symbol.iterator]().next();
  return first.done === true ? '' : first.value.segment;
}

/**
 * Up to two letters for an avatar: the first letters of the first and last words of the display
 * name ("Ada King Lovelace" → "AL"), one letter for a one-word name, else the first letter of the
 * email's local part. Upper-cased. Null when there is nothing to take them from.
 */
export function initialsOf(displayName: string | null, email: string | null): string | null {
  const words = (displayName ?? '')
    .trim()
    .split(/\s+/u)
    .filter((word) => word.length > 0);
  const first = words[0];
  const last = words.length > 1 ? words[words.length - 1] : undefined;
  if (first !== undefined) return (firstGrapheme(first) + (last === undefined ? '' : firstGrapheme(last))).toLocaleUpperCase();
  const local = (email ?? '').trim().split('@')[0] ?? '';
  return local.length > 0 ? firstGrapheme(local).toLocaleUpperCase() : null;
}
