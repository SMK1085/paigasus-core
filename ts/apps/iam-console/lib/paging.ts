// SPDX-License-Identifier: Apache-2.0
//
// Offset paging for the tenancy lists, cursor paging for the audit and dead-letters lists, and the
// dead-letters filters (spec § 5.2; SMA-629 spec § 6.3; SMA-661 spec § 4.1). IAM's tenancy list responses carry no
// total, so "Next" appears only when a page came back full. A missing or malformed offset reads as
// 0: a hand-edited query string is not an error page.
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

/**
 * The longest event-type filter the console forwards, in characters. It is the console's bound for a
 * short identifier (lib/form.ts's `slugField`). IAM matches `event_type` exactly, so the bound only
 * limits an attacker-controlled query string.
 */
export const MAX_EVENT_TYPE_LENGTH = 200;

/** What `parseEventType` answers: the filter to send to IAM ('' for none), or the error the page renders. */
export type ParsedEventType = { readonly ok: true; readonly value: string } | { readonly ok: false; readonly error: PaigasusError };

/** A filter longer than the console forwards. Like `cursorTooLong`, it never reached IAM. */
function eventTypeTooLong(): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: 'The event type filter is too long.',
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}

/**
 * The dead-letters filter (SMA-629 spec § 6.3): the first value, trimmed. `''` is no filter. A value
 * past MAX_EVENT_TYPE_LENGTH is REPORTED as invalid, never cut or dropped, for the same reason as
 * `parseCursor`: a quiet change would show the user another list than the one they asked for.
 */
export function parseEventType(raw: string | readonly string[] | undefined): ParsedEventType {
  const value = (first(raw) ?? '').trim();
  return value.length > MAX_EVENT_TYPE_LENGTH ? { ok: false, error: eventTypeTooLong() } : { ok: true, value };
}

/**
 * The longest parked-time bound the console reads, in characters (SMA-661 spec § 4.1). The longest
 * value PARKED_BOUND_PATTERN accepts, `2026-09-19T00:00:00.123456789+02:00`, has 35, so the bound
 * refuses nothing legal; it only keeps an attacker-controlled query string away from the pattern.
 * The filter input carries it as `maxLength`, which bounds TYPING only: the parser never cuts a value.
 */
export const MAX_PARKED_BOUND_LENGTH = 40;

/**
 * An ISO 8601 instant WITH a zone (SMA-661 D2, D6). It also accepts a space instead of `T`, no
 * seconds, a lower-case `z`, and up to nine fractional digits. A value with no zone is refused: the
 * console would have to guess the zone, and the guess would differ from what the operator saw. The
 * basic-format offset `+0200` is refused too, because the ECMAScript Date Time String Format does
 * not define it. The hour runs 00 to 23: ECMAScript reads 24:00 as the next day's midnight, and the
 * typed day and the canonical day would then differ.
 */
export const PARKED_BOUND_PATTERN = /^(\d{4}-\d{2}-\d{2})[T ]([01]\d|2[0-3]):(\d{2})(?::(\d{2})(?:\.(\d{1,9}))?)?(Z|z|[+-]\d{2}:\d{2})$/;

/**
 * The bound rewritten in ECMAScript's own Date Time String Format, `YYYY-MM-DDTHH:mm:ss.sssZ` or
 * `…±HH:mm`, built from the pattern's parts; null when the pattern refuses the value. `Date.parse`
 * reads only THIS string, never the operator's spelling. A space separator, a lower-case `z`, omitted
 * seconds and more than three fraction digits all lie outside that format, and `Date.parse` accepts
 * them only through implementation-specific behaviour — the same reason the basic offset `+0200` is
 * refused. Digits past the millisecond are dropped: the canonical instant has millisecond precision.
 */
export function standardInstant(value: string): string | null {
  const match = PARKED_BOUND_PATTERN.exec(value);
  if (match === null) return null;
  const [, date = '', hour = '', minute = '', second = '00', fraction = '', zone = ''] = match;
  return `${date}T${hour}:${minute}:${second}.${fraction.padEnd(3, '0').slice(0, 3)}${zone.toUpperCase()}`;
}

const PARKED_DATE = /^(\d{4})-(\d{2})-(\d{2})/;

/**
 * Whether the date part names a real calendar day. MEASURED on Node 24 (V8): `Date.parse` ACCEPTS
 * `2026-02-30T00:00:00Z` and reads it as 2026-03-02, so a finite `Date.parse` does not prove that the
 * day exists. Without this check, a typo would silently filter on another day.
 *
 * Date.UTC reads a year from 0 to 99 as 1900 to 1999, so a year from 0000 to 0099 never round-trips
 * and is refused.
 */
function isCalendarDay(value: string): boolean {
  const match = PARKED_DATE.exec(value);
  if (match === null) return false;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(Date.UTC(year, month - 1, day));
  return date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
}

/**
 * One pass of the canonicalisation: today's checks, unchanged. The pattern refuses a value with no
 * zone, which `Date.parse` reads as LOCAL time; `isCalendarDay` refuses a day that does not exist;
 * `Date.parse` refuses a time that does not exist (`00:60`).
 */
function canonicalOnce(value: string): string | null {
  if (value.length > MAX_PARKED_BOUND_LENGTH || !isCalendarDay(value)) return null;
  const standard = standardInstant(value);
  if (standard === null) return null;
  const ms = Date.parse(standard);
  return Number.isFinite(ms) ? new Date(ms).toISOString() : null;
}

/**
 * The canonical instant of a trimmed, non-empty bound, or null when the console refuses it. ONE
 * function for the GET filter (`parseParkedBound`) and the bulk form's POST field (commands.ts's
 * `parkedBoundField`), so the two cannot accept different values.
 *
 * A bound is accepted only when its canonical instant is itself accepted, so the value a link or the
 * bulk form carries always parses again; this refuses the few instants whose canonical year leaves
 * 0100–9999.
 */
export function canonicalParkedBound(value: string): string | null {
  const once = canonicalOnce(value);
  return once !== null && canonicalOnce(once) === once ? once : null;
}

/** Which parked-time input a bound came from. It only selects the error sentence. */
export type ParkedBoundField = 'parkedFrom' | 'parkedTo';

/**
 * What `parseParkedBound` answers. `raw` is the trimmed value the operator typed, so a refused bound
 * keeps it in the input (D6). `iso` is the canonical instant: the paging links, the bulk form and the
 * confirmation use it, so a link is stable. An empty filter is `{ ok: true, raw: '', iso: '' }`.
 */
export type ParsedParkedBound = { readonly ok: true; readonly raw: string; readonly iso: string } | { readonly ok: false; readonly raw: string; readonly error: PaigasusError };

const PARKED_BOUND_LABEL: Readonly<Record<ParkedBoundField, string>> = { parkedFrom: 'parked from', parkedTo: 'parked to' };

/** A bound the console refuses. Like `cursorTooLong`, it never reached IAM. */
function parkedBoundInvalid(field: ParkedBoundField): PaigasusError {
  return {
    presentation: 'invalid-input',
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    message: `The "${PARKED_BOUND_LABEL[field]}" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.`,
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
    transport: { kind: 'http', status: 400 },
  };
}

/**
 * A parked-time filter (SMA-661 spec § 4.1): the first value, trimmed. `''` is no filter. Any other
 * value must pass `canonicalParkedBound`. A refused value is REPORTED, never dropped, for the same
 * reason as `parseCursor`: a quiet change would show the operator another list than the one they
 * asked for.
 */
export function parseParkedBound(raw: string | readonly string[] | undefined, field: ParkedBoundField): ParsedParkedBound {
  const value = (first(raw) ?? '').trim();
  if (value === '') return { ok: true, raw: '', iso: '' };
  const iso = canonicalParkedBound(value);
  return iso === null ? { ok: false, raw: value, error: parkedBoundInvalid(field) } : { ok: true, raw: value, iso };
}

/**
 * `path` plus a query of the given entries, in their order (SMA-629 spec § 6.3). An entry of 0, ''
 * or null is a default and is left out, so the first unfiltered page has no query at all. A copy of
 * the gateway console's `linkHref` (ts/apps/gateway-console/lib/paging.ts:40); nothing gates a
 * divergence.
 */
export function listHref(path: string, query: Readonly<Record<string, string | number | null>>): string {
  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(query)) {
    if (value === null || value === 0 || value === '') continue;
    params.set(name, String(value));
  }
  const text = params.toString();
  return text === '' ? path : `${path}?${text}`;
}
