// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { noopLogger } from '../src/adapters/noop-logger.js';
import type { AuthLogger } from '../src/ports/logger.js';
import { sidTag } from '../src/ports/logger.js';

describe('AuthLogger', () => {
  it('the no-op default swallows events', () => {
    expect(() => noopLogger.event('login.started', { zone: 'iam' })).not.toThrow();
  });

  it('sidTag truncates to 8 characters', () => {
    expect(sidTag('abcdefghijklmnopqrstuvwxyz')).toBe('abcdefgh');
  });

  it('a recording logger captures name and fields', () => {
    const seen: Array<[string, Record<string, unknown>]> = [];
    const rec: AuthLogger = { event: (n, f) => void seen.push([n, { ...f }]) };
    rec.event('session.created', { sid: sidTag('abcdefghijkl'), zone: 'iam' });
    expect(seen).toEqual([['session.created', { sid: 'abcdefgh', zone: 'iam' }]]);
  });
});
