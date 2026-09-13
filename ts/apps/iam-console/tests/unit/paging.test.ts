// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { MAX_CURSOR_LENGTH, PAGE_SIZE, nextOffset, pageHref, parseCursor, parseOffset } from '../../lib/paging';

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
