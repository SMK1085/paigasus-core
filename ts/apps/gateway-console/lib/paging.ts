// SPDX-License-Identifier: Apache-2.0
//
// Offset paging for the gateway settings lists (SMA-636 D15). It follows the pattern of
// ts/apps/iam-console/lib/paging.ts with ONE change: a list asks IAM for PAGE_SIZE + 1 rows and
// shows PAGE_SIZE. So the page knows whether more rows exist, and a page of exactly PAGE_SIZE rows
// shows no "Next". IAM's list responses carry no total. A missing or malformed offset reads as 0.
//
// A plain module with no `server-only` import: the pager, which the client section renders, builds
// its links here.

/** The rows a list shows. IAM's server maximum is 200. */
export const PAGE_SIZE = 50;

/** The rows a list asks IAM for: one more than it shows. */
export const REQUEST_LIMIT = PAGE_SIZE + 1;

const MAX_OFFSET_DIGITS = 9;

function first(raw: string | readonly string[] | undefined): string | undefined {
  return typeof raw === 'string' ? raw : raw?.[0];
}

export function parseOffset(raw: string | readonly string[] | undefined): number {
  const value = first(raw);
  if (value === undefined || value.length > MAX_OFFSET_DIGITS || !/^\d+$/.test(value)) return 0;
  return Number(value);
}

export type Page<T> = { readonly rows: readonly T[]; readonly offset: number; readonly nextOffset: number | null };

/** One page from an answer to a REQUEST_LIMIT request. */
export function pageOf<T>(received: readonly T[], offset: number): Page<T> {
  return { rows: received.slice(0, PAGE_SIZE), offset, nextOffset: received.length > PAGE_SIZE ? offset + PAGE_SIZE : null };
}

/**
 * `path` plus a query of the given entries, in their order. An entry of 0, '' or null is a
 * default and is left out, so the first page of a list has no offset in its URL.
 */
export function linkHref(path: string, query: Readonly<Record<string, string | number | null>>): string {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === null || value === 0 || value === '') continue;
    params.set(name, String(value));
  }
  const text = params.toString();
  return text === '' ? path : `${path}?${text}`;
}
