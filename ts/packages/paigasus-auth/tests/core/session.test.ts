// SPDX-License-Identifier: Apache-2.0
import { describe, expect, expectTypeOf, it } from 'vitest';
import { SESSION_VIEW_KEYS, toSessionView, isSessionRecord } from '../../src/core/session.js';
import type { SessionRecord, SessionView } from '../../src/core/session.js';
import { makeRecord } from '../store-contract.js';

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

  it('deep-copies roleGrants rather than handing out the record array or its elements by reference', () => {
    // Task 10: a client surface now holds a SessionView in-process, so mutating the view's
    // `grants` array — or a field on one of its elements — must never reach back into the
    // SessionRecord (or a shared array/element between two views built from the same record).
    const v = toSessionView(RECORD);
    expect(v.grants).not.toBe(RECORD.principal.roleGrants);
    expect(v.grants[0]).not.toBe(RECORD.principal.roleGrants[0]);

    v.grants.push({ scopePrn: 'prn:pgs:iam::org1:org/injected', roleKey: 'intruder' });
    expect(RECORD.principal.roleGrants).toHaveLength(1);

    // Review round 1: a shallow `[...roleGrants]` still passes the array-identity checks above
    // while leaving each ELEMENT aliased — this is the check that catches that specifically.
    const grant = v.grants[0];
    if (grant === undefined) throw new Error('expected the seeded grant to survive the copy');
    grant.roleKey = 'tampered';
    expect(RECORD.principal.roleGrants[0]?.roleKey).toBe('admin');
  });

  it('a SessionRecord is not assignable to a SessionView', () => {
    // Enforced by `tsc --noEmit` in the build/typecheck tasks, NOT by `vitest run` — the inherited
    // test task passes no --typecheck, so expectTypeOf is a runtime no-op there. It works only
    // because tsconfig.json includes tests/**/*. Do not "simplify" either half.
    expectTypeOf<SessionRecord>().not.toMatchTypeOf<SessionView>();
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-626 § 4.2. A `value is SessionRecord` predicate can drift from the type in two directions,
// and TypeScript reports neither: under-checking compiles silently and lets a poisoned record
// through, over-checking makes MemorySessionStore.get return null for a VALID record — which a
// user reads as "I was logged out", not as a type error. The field table below is the control on
// both directions: every required field must make the predicate false when it is missing, and the
// complete record must make it true.
// ---------------------------------------------------------------------------------------------
describe('isSessionRecord (SMA-626 § 4.2)', () => {
  it('accepts a complete record', () => {
    expect(isSessionRecord(makeRecord())).toBe(true);
  });

  it('accepts a record with no refreshToken (the field is optional)', () => {
    const rec: Record<string, unknown> = { ...makeRecord() };
    delete rec['refreshToken'];
    expect(isSessionRecord(rec)).toBe(true);
  });

  // `exactOptionalPropertyTypes`: an ABSENT refreshToken is legal, an explicit null is not.
  it('rejects an explicit null refreshToken', () => {
    expect(isSessionRecord({ ...makeRecord(), refreshToken: null })).toBe(false);
  });

  it.each(['version', 'rev', 'accessToken', 'accessExpiresAt', 'absoluteExpiresAt', 'idTokenClaims', 'principal'])('rejects a record missing %s', (field) => {
    const rec: Record<string, unknown> = { ...makeRecord() };
    delete rec[field];
    expect(isSessionRecord(rec)).toBe(false);
  });

  it.each(['iss', 'sub'])('rejects a record whose idTokenClaims is missing %s', (claim) => {
    const claims: Record<string, unknown> = { ...makeRecord().idTokenClaims };
    delete claims[claim];
    expect(isSessionRecord({ ...makeRecord(), idTokenClaims: claims })).toBe(false);
  });

  it.each(['principalPrn', 'issuer', 'subject', 'memberships', 'roleGrants', 'grantsAvailable'])('rejects a record whose principal is missing %s', (field) => {
    const principal: Record<string, unknown> = { ...makeRecord().principal };
    delete principal[field];
    expect(isSessionRecord({ ...makeRecord(), principal })).toBe(false);
  });

  // HOLE 3 (§ 4.1), stated as its own test because it is the one with a security consequence:
  // can() FAILS OPEN on grantsAvailable (src/client.ts:67), so a principal missing that field
  // makes every browser-side capability check return true.
  it('rejects a principal shaped { roleGrants: [] } — the can() fail-open case', () => {
    expect(isSessionRecord({ ...makeRecord(), principal: { roleGrants: [] } })).toBe(false);
  });

  // HOLE 1 (§ 4.1): JSON.parse('null') SUCCEEDS, so the parse guard never fires on this one.
  it.each([null, undefined, 3, 'a string', [], true])('rejects the non-object value %p', (value) => {
    expect(isSessionRecord(value)).toBe(false);
  });

  // HOLE 2 (§ 4.1): two NaN comparisons in resolveSession let this through as a LIVE session
  // carrying accessToken: undefined.
  it('rejects a body of { version: 1 } and nothing else', () => {
    expect(isSessionRecord({ version: 1 })).toBe(false);
  });

  it('rejects a version other than 1', () => {
    expect(isSessionRecord({ ...makeRecord(), version: 2 })).toBe(false);
  });

  it.each(['rev', 'accessExpiresAt', 'absoluteExpiresAt'])('rejects a non-finite %s', (field) => {
    expect(isSessionRecord({ ...makeRecord(), [field]: Number.NaN })).toBe(false);
  });

  it('rejects a roleGrants element that is not a { scopePrn, roleKey } object', () => {
    const principal = { ...makeRecord().principal, roleGrants: ['not-an-object'] };
    expect(isSessionRecord({ ...makeRecord(), principal })).toBe(false);
  });
});
