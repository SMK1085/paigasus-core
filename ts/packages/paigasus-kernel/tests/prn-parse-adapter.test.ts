// SPDX-License-Identifier: Apache-2.0
//
// The typed adapter over the six-string wire form (SMA-673 D2). It checks the wire CONVENTION only
// (six strings; an error row has five empty fields), never PRN grammar: the corpus replays prove
// the grammar.
import { describe, expect, it } from 'vitest';
import { toPrnParseResult } from '../src/prn-parse';

describe('toPrnParseResult', () => {
  it('maps each position of a valid row to its own name', () => {
    // Every position holds a distinct value, so a swap of any two names is visible.
    expect(toPrnParseResult(['', 'svc', 'reg', 'org', 'type', 'id'])).toEqual({ ok: true, service: 'svc', region: 'reg', org: 'org', resourceType: 'type', resourceId: 'id' });
  });

  it('maps an error row to its error kind only', () => {
    expect(toPrnParseResult(['bad-org', '', '', '', '', ''])).toEqual({ ok: false, errorKind: 'bad-org' });
  });

  it.each([[[]], [['', 'a', 'b', 'c', 'd']], [['', 'a', 'b', 'c', 'd', 'e', 'f']]])('throws a TypeError for a wire array of the wrong length (%j)', (wire) => {
    expect(() => toPrnParseResult(wire)).toThrow(TypeError);
  });

  it('throws a TypeError for a non-string element', () => {
    expect(() => toPrnParseResult(['', 'svc', 'reg', 5, 'type', 'id'])).toThrow(TypeError);
  });

  it.each([1, 2, 3, 4, 5])('throws a TypeError for an error row with a non-empty field at position %i', (position) => {
    const wire = ['bad-org', '', '', '', '', ''];
    wire[position] = 'x';
    expect(() => toPrnParseResult(wire)).toThrow(TypeError);
  });

  it('does not put a field value into the error message', () => {
    expect(() => toPrnParseResult(['bad-org', 'secret-value', '', '', '', ''])).toThrow(/^(?!.*secret-value)/s);
  });
});
