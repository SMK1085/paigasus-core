// SPDX-License-Identifier: Apache-2.0
//
// The gateway settings paging (SMA-636 D15). A list asks IAM for PAGE_SIZE + 1 rows and shows
// PAGE_SIZE, so a page of EXACTLY PAGE_SIZE rows shows no "Next" and no "more exist" note.
import { describe, expect, it } from 'vitest';
import { PAGE_SIZE, REQUEST_LIMIT, linkHref, pageOf, parseOffset } from '../../lib/paging';

const items = (count: number): number[] => Array.from({ length: count }, (_, index) => index);

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

describe('pageOf', () => {
  it('asks IAM for one row more than it shows', () => {
    expect(PAGE_SIZE).toBe(50);
    expect(REQUEST_LIMIT).toBe(51);
  });

  it('shows exactly 50 rows and no next page when IAM sent exactly 50', () => {
    const page = pageOf(items(50), 0);
    expect(page.rows).toHaveLength(50);
    expect(page.nextOffset).toBeNull();
  });

  it('shows 50 rows and a next page when IAM sent 51', () => {
    const page = pageOf(items(51), 100);
    expect(page.rows).toEqual(items(50));
    expect(page.offset).toBe(100);
    expect(page.nextOffset).toBe(150);
  });

  it('has no next page for an empty answer', () => {
    expect(pageOf([], 0)).toEqual({ rows: [], offset: 0, nextOffset: null });
  });
});

describe('linkHref', () => {
  it('drops a default value (0, empty text, null) and keeps the order of the rest', () => {
    expect(linkHref('/gateway/orgs/o', { saOffset: 0, sa: null, keyOffset: 50 })).toBe('/gateway/orgs/o?keyOffset=50');
    expect(linkHref('/gateway/orgs/o', { sa: 'abc', saOffset: 50 })).toBe('/gateway/orgs/o?sa=abc&saOffset=50');
  });

  it('is the bare path when nothing is left', () => {
    expect(linkHref('/gateway/orgs/o', { saOffset: 0, sa: '' })).toBe('/gateway/orgs/o');
  });

  it('lets the parameter a pager sets override a kept entry of the same name', () => {
    const keep = { saOffset: 50, sa: 'abc' };
    const param: string = 'saOffset';
    expect(linkHref('/p', { ...keep, [param]: 100 })).toBe('/p?saOffset=100&sa=abc');
  });
});
