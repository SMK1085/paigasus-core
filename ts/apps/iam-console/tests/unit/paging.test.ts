// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { PAGE_SIZE, nextOffset, pageHref, parseCursor, parseOffset } from '../../lib/paging';

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
  it('passes an opaque cursor through and drops an absent or oversized one', () => {
    expect(parseCursor('abc')).toBe('abc');
    expect(parseCursor(['abc', 'def'])).toBe('abc');
    expect(parseCursor(undefined)).toBe('');
    expect(parseCursor('x'.repeat(1025))).toBe('');
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
