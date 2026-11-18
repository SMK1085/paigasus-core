// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { initialsOf } from '../../src/shell/initials';

describe('initialsOf (SMA-783)', () => {
  it.each([
    ['Ada Lovelace', 'AL'],
    ['Ada King Lovelace', 'AL'],
    ['ada lovelace', 'AL'],
    ['  Ada   Lovelace  ', 'AL'],
    ['Plato', 'P'],
    ['Émile Zola', 'ÉZ'],
    ['Émile Zola', 'ÉZ'],
    ['Zoë Øster', 'ZØ'],
    ['Ἀριστοτέλης', 'Ἀ'],
  ])('%j → %j', (displayName, expected) => {
    expect(initialsOf(displayName, 'someone@example.com')).toBe(expected);
  });

  it('falls back to the first letter of the email, then to null', () => {
    expect(initialsOf(null, 'ada@example.com')).toBe('A');
    expect(initialsOf('   ', 'ada@example.com')).toBe('A');
    expect(initialsOf(null, '@example.com')).toBeNull();
    expect(initialsOf(null, null)).toBeNull();
  });
});
