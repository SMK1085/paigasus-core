// SPDX-License-Identifier: Apache-2.0
//
// Offset paging for the tenancy lists and cursor paging for the audit list (spec § 5.2). IAM's
// tenancy list responses carry no total, so "Next" appears only when a page came back full. A
// missing or malformed offset reads as 0: a hand-edited query string is not an error page.
import 'server-only';

/** IAM's server maximum is 200; the console asks for 50. */
export const PAGE_SIZE = 50;

const MAX_OFFSET_DIGITS = 9;
const MAX_CURSOR_LENGTH = 1024;

function first(raw: string | readonly string[] | undefined): string | undefined {
  return typeof raw === 'string' ? raw : raw?.[0];
}

export function parseOffset(raw: string | readonly string[] | undefined): number {
  const value = first(raw);
  if (value === undefined || value.length > MAX_OFFSET_DIGITS || !/^\d+$/.test(value)) return 0;
  return Number(value);
}

export function nextOffset(offset: number, received: number): number | null {
  return received >= PAGE_SIZE ? offset + PAGE_SIZE : null;
}

/**
 * The href of one page link. `keep` holds the offsets of the OTHER lists on the same page, so paging
 * one list does not reset the others. A kept offset of 0 is the default page and is left out. `param`
 * is set last, so it overrides a kept entry of the same name.
 */
export function pageHref(path: string, param: string, offset: number, keep: Readonly<Record<string, number>> = {}): string {
  const query = new URLSearchParams();
  for (const [name, value] of Object.entries(keep)) {
    if (value > 0) query.set(name, String(value));
  }
  query.set(param, String(offset));
  return `${path}?${query.toString()}`;
}

/** The audit cursor is opaque. IAM validates it and answers `invalid-cursor` for a bad one. */
export function parseCursor(raw: string | readonly string[] | undefined): string {
  const value = first(raw);
  return value === undefined || value.length > MAX_CURSOR_LENGTH ? '' : value;
}
