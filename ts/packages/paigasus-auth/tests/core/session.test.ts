// SPDX-License-Identifier: Apache-2.0
import { describe, expect, expectTypeOf, it } from 'vitest';
import { SESSION_VIEW_KEYS, toSessionView } from '../../src/core/session.js';
import type { SessionRecord, SessionView } from '../../src/core/session.js';

const RECORD: SessionRecord = {
  version: 1,
  rev: 3,
  accessToken: 'AT-secret',
  refreshToken: 'RT-secret',
  accessExpiresAt: 2_000_000,
  absoluteExpiresAt: 9_000_000,
  idTokenClaims: { iss: 'https://idp', sub: 'u1', email: 'a@b.c', name: 'Alice' },
  principal: {
    principalPrn: 'prn:pgs:iam::org1:user/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8',
    issuer: 'https://idp',
    subject: 'u1',
    memberships: [],
    roleGrants: [{ scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' }],
    grantsAvailable: false,
  },
};

describe('toSessionView', () => {
  it('emits exactly the pinned key set', () => {
    // STRICT EQUALITY, and it is the point. Widening what reaches the browser must be a
    // deliberate edit to this list, never a side effect of adding a field to SessionRecord.
    expect(Object.keys(toSessionView(RECORD)).sort()).toEqual([...SESSION_VIEW_KEYS].sort());
  });

  it('carries no token field under any name', () => {
    const serialised = JSON.stringify(toSessionView(RECORD));
    expect(serialised).not.toContain('AT-secret');
    expect(serialised).not.toContain('RT-secret');
    for (const k of Object.keys(toSessionView(RECORD))) {
      expect(k.toLowerCase()).not.toContain('token');
    }
  });

  it('projects identity and grants', () => {
    const v = toSessionView(RECORD);
    expect(v.principalPrn).toBe(RECORD.principal.principalPrn);
    expect(v.displayName).toBe('Alice');
    expect(v.email).toBe('a@b.c');
    expect(v.grants).toEqual(RECORD.principal.roleGrants);
    expect(v.grantsAvailable).toBe(false);
  });

  it('falls back to the email local part when no name claim is present', () => {
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    const { name: _name, ...claims } = RECORD.idTokenClaims;
    expect(toSessionView({ ...RECORD, idTokenClaims: claims }).displayName).toBe('a');
  });

  it('copies roleGrants rather than handing out the record array by reference', () => {
    // Task 10: a client surface now holds a SessionView in-process, so mutating the view's
    // `grants` array must never reach back into the SessionRecord (or a shared array between two
    // views built from the same record).
    const v = toSessionView(RECORD);
    expect(v.grants).not.toBe(RECORD.principal.roleGrants);
    v.grants.push({ scopePrn: 'prn:pgs:iam::org1:org/injected', roleKey: 'intruder' });
    expect(RECORD.principal.roleGrants).toHaveLength(1);
  });

  it('a SessionRecord is not assignable to a SessionView', () => {
    // Enforced by `tsc --noEmit` in the build/typecheck tasks, NOT by `vitest run` — the inherited
    // test task passes no --typecheck, so expectTypeOf is a runtime no-op there. It works only
    // because tsconfig.json includes tests/**/*. Do not "simplify" either half.
    expectTypeOf<SessionRecord>().not.toMatchTypeOf<SessionView>();
  });
});
