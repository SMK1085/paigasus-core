// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import {
  MAX_CURSOR_LENGTH,
  MAX_EVENT_TYPE_LENGTH,
  MAX_PARKED_BOUND_LENGTH,
  PAGE_SIZE,
  PARKED_BOUND_PATTERN,
  canonicalParkedBound,
  listHref,
  nextOffset,
  pageHref,
  parseCursor,
  parseEventType,
  parseOffset,
  parseParkedBound,
  standardInstant,
} from '../../lib/paging';

describe('parseOffset', () => {
  it('reads a plain non-negative integer', () => {
    expect(parseOffset('0')).toBe(0);
    expect(parseOffset('50')).toBe(50);
    expect(parseOffset(['100', '150'])).toBe(100);
  });

  it('reads anything else as 0, because a hand-edited query string is not worth an error page', () => {
    for (const raw of [undefined, '', '-50', '5.5', '1e3', 'abc', ' 50', '9999999999']) {
      expect(parseOffset(raw)).toBe(0);
    }
  });
});

describe('nextOffset', () => {
  it('offers a next page only when the page came back full, because IAM reports no total', () => {
    expect(nextOffset(0, PAGE_SIZE)).toBe(PAGE_SIZE);
    expect(nextOffset(50, PAGE_SIZE)).toBe(100);
    expect(nextOffset(0, PAGE_SIZE - 1)).toBeNull();
    expect(nextOffset(0, 0)).toBeNull();
  });
});

describe('parseCursor', () => {
  it('passes an opaque cursor through, because IAM owns the cursor grammar', () => {
    expect(parseCursor('abc')).toEqual({ ok: true, cursor: 'abc' });
    expect(parseCursor(['abc', 'def'])).toEqual({ ok: true, cursor: 'abc' });
    expect(parseCursor(undefined)).toEqual({ ok: true, cursor: '' });
    expect(parseCursor('x'.repeat(MAX_CURSOR_LENGTH))).toEqual({ ok: true, cursor: 'x'.repeat(MAX_CURSOR_LENGTH) });
  });

  // THE DEFECT THIS PINS (review, defect 5). An over-long cursor used to become `''`, which is the
  // FIRST page: the user asked for one page and silently got another, and the only answer that
  // looks like success is the wrong one. The bound stays — a query string is attacker-controlled —
  // but it now reports the cursor as invalid instead of resetting it.
  it('reports a cursor past the bound as invalid input, and never as page one', () => {
    const parsed = parseCursor('x'.repeat(MAX_CURSOR_LENGTH + 1));

    expect(parsed.ok).toBe(false);
    if (parsed.ok) throw new Error('expected an invalid cursor');
    expect(parsed.error.presentation).toBe('invalid-input');
    // It never reached IAM, so it carries no IAM data — the lib/form.ts local-error rule.
    expect(parsed.error.correlationId).toBeNull();
    expect(parsed.error.reason).toBeNull();
  });
});

describe('pageHref', () => {
  it('puts the offset into the one named parameter', () => {
    expect(pageHref('/iam/orgs', 'offset', 50)).toBe('/iam/orgs?offset=50');
    expect(pageHref('/iam/orgs', 'offset', 0)).toBe('/iam/orgs?offset=0');
  });

  it("keeps the other list's offset, so paging one list does not reset the other", () => {
    expect(pageHref('/iam/orgs/x', 'moffset', 50, { offset: 100 })).toBe('/iam/orgs/x?offset=100&moffset=50');
  });

  it('drops a kept offset of 0, because 0 is the default page', () => {
    expect(pageHref('/iam/orgs/x', 'moffset', 50, { offset: 0 })).toBe('/iam/orgs/x?moffset=50');
  });

  it('lets the parameter override a kept entry of the same name', () => {
    expect(pageHref('/iam/orgs', 'offset', 50, { offset: 100 })).toBe('/iam/orgs?offset=50');
  });
});

// SMA-629 spec § 6.3. The dead-letters filter: IAM matches event_type exactly, and '' is no filter.
describe('parseEventType', () => {
  it('reads the first value, trimmed, and no value as no filter', () => {
    expect(parseEventType('iam.team.created')).toEqual({ ok: true, value: 'iam.team.created' });
    expect(parseEventType(['iam.team.created', 'iam.project.created'])).toEqual({ ok: true, value: 'iam.team.created' });
    expect(parseEventType('  iam.team.created  ')).toEqual({ ok: true, value: 'iam.team.created' });
    expect(parseEventType(undefined)).toEqual({ ok: true, value: '' });
    expect(parseEventType('')).toEqual({ ok: true, value: '' });
    expect(parseEventType('   ')).toEqual({ ok: true, value: '' });
  });

  it('accepts 200 characters after the trim and refuses 201 as invalid input that never reached IAM', () => {
    expect(MAX_EVENT_TYPE_LENGTH).toBe(200);
    expect(parseEventType(` ${'e'.repeat(200)} `)).toEqual({ ok: true, value: 'e'.repeat(200) });

    const parsed = parseEventType('e'.repeat(201));

    expect(parsed.ok).toBe(false);
    if (parsed.ok) throw new Error('expected an invalid event type');
    expect(parsed.error.presentation).toBe('invalid-input');
    expect(parsed.error.correlationId).toBeNull();
    expect(parsed.error.reason).toBeNull();
  });
});

// SMA-661 spec § 4.1. A parked-time bound: an ISO instant WITH a zone, in four more spellings (D6).
// '' is no filter. A refused bound echoes the typed value, so the filter form can keep it (D6).
describe('parseParkedBound', () => {
  const CANONICAL = '2026-09-19T00:00:00.000Z';

  it('reads no value, an empty value and blanks as no filter', () => {
    for (const raw of [undefined, '', '   ']) expect(parseParkedBound(raw, 'parkedFrom')).toEqual({ ok: true, raw: '', iso: '' });
  });

  it.each([
    ['the canonical Z form', '2026-09-19T00:00:00Z', CANONICAL],
    ['a lower-case z', '2026-09-19T00:00:00z', CANONICAL],
    ['a space instead of T', '2026-09-19 00:00:00Z', CANONICAL],
    ['no seconds', '2026-09-19T00:00Z', CANONICAL],
    ['fractional seconds, cut to milliseconds', '2026-09-19T00:00:00.123456789Z', '2026-09-19T00:00:00.123Z'],
    ['a +02:00 offset', '2026-09-19T02:00:00+02:00', CANONICAL],
    ['the longest legal value, 35 characters', '2026-09-19T02:00:00.123456789+02:00', '2026-09-19T00:00:00.123Z'],
  ])('accepts %s and normalises it', (_label, raw, iso) => {
    expect(parseParkedBound(raw, 'parkedFrom')).toEqual({ ok: true, raw, iso });
  });

  it('reads the first value, trims it, and echoes the TRIMMED value as raw', () => {
    expect(parseParkedBound(['2026-09-19T00:00:00Z', 'not a time'], 'parkedTo')).toEqual({ ok: true, raw: '2026-09-19T00:00:00Z', iso: CANONICAL });
    expect(parseParkedBound('  2026-09-19T00:00:00Z  ', 'parkedTo')).toEqual({ ok: true, raw: '2026-09-19T00:00:00Z', iso: CANONICAL });
  });

  it.each([
    ['a basic-format offset', '2026-09-19T00:00:00+0200'],
    ['no zone', '2026-09-19T00:00:00'],
    ['a date only', '2026-09-19'],
    ['a day that does not exist', '2026-02-30T00:00:00Z'],
    ['a minute that does not exist', '2026-09-19T00:60:00Z'],
    ['an hour that does not exist', '2026-09-19T24:00:00Z'],
    ['words', 'not a time'],
    ['41 characters', '9'.repeat(41)],
  ])('refuses %s, echoes it back, and never reached IAM', (_label, raw) => {
    const parsed = parseParkedBound(raw, 'parkedFrom');

    expect(parsed.ok).toBe(false);
    if (parsed.ok) throw new Error('expected a refused bound');
    expect(parsed.raw).toBe(raw);
    expect(parsed.error.presentation).toBe('invalid-input');
    expect(parsed.error.correlationId).toBeNull();
    expect(parsed.error.reason).toBeNull();
  });

  it('names the field in the sentence', () => {
    const from = parseParkedBound('2026-09-19', 'parkedFrom');
    const to = parseParkedBound('2026-09-19', 'parkedTo');

    if (from.ok || to.ok) throw new Error('expected two refused bounds');
    expect(from.error.message).toBe('The "parked from" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.');
    expect(to.error.message).toBe('The "parked to" filter is not a valid time. Use an ISO instant with a zone, for example 2026-09-19T00:00:00Z.');
  });

  // THE REASON FOR THE CALENDAR CHECK, measured on Node 24 (V8). The pattern accepts 2026-02-30, and
  // so does Date.parse: it reads the value as 2026-03-02. The spec expected Date.parse to refuse it.
  it('does not trust Date.parse with a day that does not exist', () => {
    expect(PARKED_BOUND_PATTERN.test('2026-02-30T00:00:00Z')).toBe(true);
    expect(new Date('2026-02-30T00:00:00Z').toISOString()).toBe('2026-03-02T00:00:00.000Z');
    expect(canonicalParkedBound('2026-02-30T00:00:00Z')).toBeNull();
  });

  // Date.parse sees only the standard format. V8 happens to accept every D6 spelling raw as well, so
  // canonicalParkedBound's output cannot show which string it parsed; this case pins the rewrite.
  it.each([
    ['2026-09-19T00:00:00Z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19 00:00:00Z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19T00:00Z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19T00:00:00z', '2026-09-19T00:00:00.000Z'],
    ['2026-09-19T00:00:00.5+02:00', '2026-09-19T00:00:00.500+02:00'],
    ['2026-09-19T00:00:00.123456789Z', '2026-09-19T00:00:00.123Z'],
  ])('rewrites %s as the standard %s before Date.parse reads it', (value, standard) => {
    expect(standardInstant(value)).toBe(standard);
  });

  it('rewrites nothing the pattern refuses', () => {
    expect(standardInstant('2026-09-19T00:00:00')).toBeNull();
    expect(standardInstant('2026-09-19T00:00:00+0200')).toBeNull();
    expect(standardInstant('2026-09-19T24:00:00Z')).toBeNull();
  });

  it('bounds the length at 40, above the 35 characters of the longest legal value', () => {
    expect(MAX_PARKED_BOUND_LENGTH).toBe(40);
    expect('2026-09-19T02:00:00.123456789+02:00'.length).toBe(35);
  });
});

describe('listHref', () => {
  it('leaves out every empty value, so the first unfiltered page has no query', () => {
    expect(listHref('/iam/dead-letters', {})).toBe('/iam/dead-letters');
    expect(listHref('/iam/dead-letters', { eventType: '', cursor: null })).toBe('/iam/dead-letters');
    expect(listHref('/iam/dead-letters', { eventType: 'iam.team.created', cursor: '' })).toBe('/iam/dead-letters?eventType=iam.team.created');
  });

  it('encodes the values and keeps their order', () => {
    expect(listHref('/iam/dead-letters', { eventType: 'a b', cursor: 'c&d=e' })).toBe('/iam/dead-letters?eventType=a+b&cursor=c%26d%3De');
  });

  it('carries the canonical parked bounds and leaves an empty one out (SMA-661 § 4.5)', () => {
    expect(listHref('/iam/dead-letters', { eventType: '', parkedFrom: '2026-09-19T00:00:00.000Z', parkedTo: '' })).toBe('/iam/dead-letters?parkedFrom=2026-09-19T00%3A00%3A00.000Z');
    expect(listHref('/iam/dead-letters', { eventType: 'a', parkedFrom: '', parkedTo: '2026-09-20T00:00:00.000Z', cursor: 'c2' })).toBe(
      '/iam/dead-letters?eventType=a&parkedTo=2026-09-20T00%3A00%3A00.000Z&cursor=c2',
    );
  });
});
