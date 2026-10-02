// SPDX-License-Identifier: Apache-2.0
//
// forwardableHeaders (SMA-640 spec § 6.1). Rows 3, 4 and 9 vary case and white space on purpose:
// a fixture that uses only lower case and no spaces inherits the implementer's assumption, and a
// missing case fold or trim then passes every row.
//
// The fixed list below spells 'proxy-connection'. That is why the AC4 grep in the plan excludes
// tests/ (plan decision P1).
import type { IncomingHttpHeaders } from 'node:http';
import { describe, expect, it } from 'vitest';
import { forwardableHeaders } from '../../testing/index';

const FIXED = ['connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization', 'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade'] as const;

describe('forwardableHeaders', () => {
  // Row 1. One named row per fixed name, so a deletion from the set reds a row that says which.
  it.each(FIXED)('removes the fixed hop-by-hop field %s', (name) => {
    const out = forwardableHeaders({ [name]: 'v', 'x-kept': '1' });
    expect(out).not.toHaveProperty(name);
    expect(out['x-kept']).toBe('1');
  });

  // Row 2.
  it('removes a field that Connection nominates and keeps the others', () => {
    const out = forwardableHeaders({ connection: 'x-internal', 'x-internal': '1', 'x-other': '2' });
    expect(out).not.toHaveProperty('x-internal');
    expect(out['x-other']).toBe('2');
  });

  // Row 3.
  it('folds case and strips spaces and tabs around each nominated token', () => {
    const out = forwardableHeaders({ connection: ' X-Internal ,\tX-Other ', 'x-internal': '1', 'x-other': '2', 'x-kept': '3' });
    expect(out).not.toHaveProperty('x-internal');
    expect(out).not.toHaveProperty('x-other');
    expect(out['x-kept']).toBe('3');
  });

  // Row 4. Node joins repeated Connection lines with ", " and keeps the sender's case and spaces
  // (spec § 6.6).
  it('reads every token of joined Connection lines', () => {
    const out = forwardableHeaders({ connection: 'x-a, X-B , close', 'x-a': '1', 'x-b': '2', 'x-kept': '3' });
    expect(out).not.toHaveProperty('x-a');
    expect(out).not.toHaveProperty('x-b');
    expect(out['x-kept']).toBe('3');
  });

  // Row 5. @types/node types `connection` as `string | undefined`, so an array needs a cast. The
  // helper accepts one as defence in depth, for a caller that builds headers by hand.
  it('reads a Connection value that is an array', () => {
    const headers = { connection: ['x-a', 'X-B'], 'x-a': '1', 'x-b': '2', 'x-kept': '3' } as unknown as IncomingHttpHeaders;
    const out = forwardableHeaders(headers);
    expect(out).not.toHaveProperty('x-a');
    expect(out).not.toHaveProperty('x-b');
    expect(out['x-kept']).toBe('3');
  });

  // Row 6. NOT a mutation target: no header name is empty, so no change to token handling can make
  // this row fail. It checks only that the helper does not throw and keeps every other field.
  it('does not throw on empty tokens and keeps every other field', () => {
    const out = forwardableHeaders({ connection: ' , ,', 'x-a': '1', host: 'h' });
    expect(out).toEqual({ 'x-a': '1', host: 'h' });
  });

  // Row 7.
  it('drops undefined values and passes a set-cookie array through unchanged', () => {
    const cookies = ['a=1; Path=/', 'b=2; Path=/'];
    const out = forwardableHeaders({ 'x-gone': undefined, 'set-cookie': cookies });
    expect(out).not.toHaveProperty('x-gone');
    expect(out['set-cookie']).toEqual(cookies);
  });

  // Row 8.
  it('does not change the input object', () => {
    const input: IncomingHttpHeaders = { connection: 'x-a', 'x-a': '1', 'keep-alive': 'timeout=5' };
    const before = structuredClone(input);
    forwardableHeaders(input);
    expect(input).toEqual(before);
  });

  // Row 9, decision D6. In Node the outgoing headers object IS the request framing, so removing a
  // nominated content-length would let a GET body reach the upstream as a second request (spec § 6.5).
  it('keeps content-length when Connection nominates it', () => {
    expect(forwardableHeaders({ connection: 'content-length', 'content-length': '3' })['content-length']).toBe('3');
  });

  it('keeps content-length and still removes the other tokens when the nomination varies case', () => {
    const out = forwardableHeaders({ connection: 'Content-Length, x-a', 'content-length': '3', 'x-a': '1' });
    expect(out['content-length']).toBe('3');
    expect(out).not.toHaveProperty('x-a');
  });
});
