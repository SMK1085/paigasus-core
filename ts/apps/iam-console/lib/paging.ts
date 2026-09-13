// SPDX-License-Identifier: Apache-2.0
//
// Offset paging for the tenancy lists and cursor paging for the audit list (spec § 5.2). IAM's
// tenancy list responses carry no total, so "Next" appears only when a page came back full. A
// missing or malformed offset reads as 0: a hand-edited query string is not an error page.
import 'server-only';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

/** IAM's server maximum is 200; the console asks for 50. */
export const PAGE_SIZE = 50;

const MAX_OFFSET_DIGITS = 9;

/**
 * The longest cursor the console forwards to IAM, in characters. IAM's own cursors are far shorter;
 * this bounds an attacker-controlled query string, nothing else. A longer value is REPORTED as
 * invalid (see `parseCursor`), never quietly replaced.
 */
export const MAX_CURSOR_LENGTH = 1024;

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

/** What `parseCursor` answers: the cursor to send to IAM, or the error the page renders instead. */
export type ParsedCursor = { readonly ok: true; readonly cursor: string } | { readonly ok: false; readonly error: PaigasusError };

/**
 * A cursor that is longer than the console will forward. Like `lib/form.ts`'s two local errors it
 * never reached IAM, so it carries no IAM data: no domain, no reason, no correlation id. The
 * `transport` field says HTTP 400 because the BFF itself refused the request; nothing branches on
 * it (ADR-0019 E8).
 */
function cursorTooLong(): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'The page cursor is not valid.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}

/**
 * The audit cursor is opaque: IAM issues it, IAM validates it, and IAM answers `invalid-cursor` for
 * a bad one. So this hands the value to IAM UNCHANGED rather than judging it here — an absent
 * cursor is the first page, and every other value is IAM's to accept or refuse.
 *
 * THE ONE EXCEPTION IS LENGTH (review, defect 5). A query string is attacker-controlled and
 * unbounded, so a cursor past MAX_CURSOR_LENGTH is not forwarded. It used to become `''` instead,
 * which is the FIRST page: the user asked for one page, silently got another, and the doc comment
 * promising that IAM validates the cursor was false for exactly that input. It is now reported as
 * invalid input, so the page shows an error rather than the wrong page.
 */
export function parseCursor(raw: string | readonly string[] | undefined): ParsedCursor {
  const value = first(raw);
  if (value === undefined) return { ok: true, cursor: '' };
  return value.length > MAX_CURSOR_LENGTH ? { ok: false, error: cursorTooLong() } : { ok: true, cursor: value };
}
