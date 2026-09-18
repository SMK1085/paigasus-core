// SPDX-License-Identifier: Apache-2.0
//
// API key facts as the screens show them (SMA-636 spec § 5.4, § 5.7). The clock is injected, so
// every case is exact.
import { describe, expect, it } from 'vitest';
import { ApiKeyStatus } from '@paigasus/sdk/iam/types';
import { expiresAtFor, formatDate, keyStatus, timestampMs } from '../../app/(console)/service-accounts/keys';
import { EXPIRY_CHOICES, EXPIRY_LABEL, KEY_STATUS_LABEL } from '../../app/(console)/service-accounts/view';

const NOW = Date.UTC(2026, 8, 18, 12, 0, 0);
const DAY = 86_400_000;

describe('keyStatus (§ 5.7, in this order)', () => {
  it('makes every key inactive when the account is not active, whatever IAM says', () => {
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: null }, false, NOW)).toBe('inactive');
    expect(keyStatus({ status: ApiKeyStatus.REVOKED, expiresAtMs: NOW - DAY }, false, NOW)).toBe('inactive');
  });

  it('reads a revoked key as revoked, also when it expired', () => {
    expect(keyStatus({ status: ApiKeyStatus.REVOKED, expiresAtMs: NOW - DAY }, true, NOW)).toBe('revoked');
  });

  it('reads a key past its expiry as expired, and the expiry instant itself as past', () => {
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: NOW - 1 }, true, NOW)).toBe('expired');
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: NOW }, true, NOW)).toBe('expired');
  });

  it('reads a key with no expiry or a future expiry as active', () => {
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: null }, true, NOW)).toBe('active');
    expect(keyStatus({ status: ApiKeyStatus.ACTIVE, expiresAtMs: NOW + 1 }, true, NOW)).toBe('active');
  });

  it('reads a status this build does not know as active: IAM checks every call itself', () => {
    expect(keyStatus({ status: 9 as ApiKeyStatus, expiresAtMs: null }, true, NOW)).toBe('active');
  });
});

describe('expiresAtFor (§ 5.4, D7)', () => {
  it('sends no expires_at for the IAM default', () => {
    expect(expiresAtFor('default', NOW)).toBeUndefined();
  });

  it.each([
    ['30', 30],
    ['90', 90],
    ['365', 365],
  ] as const)('computes %s days from the injected clock, in whole seconds', (choice, days) => {
    expect(expiresAtFor(choice, NOW + 999)).toEqual({ seconds: BigInt(Math.floor((NOW + 999 + days * DAY) / 1000)), nanos: 0 });
  });
});

describe('timestamps and dates', () => {
  it('reads a Timestamp as milliseconds, and an unset one as null', () => {
    expect(timestampMs({ seconds: 1_788_000_000n, nanos: 500_000_000 })).toBe(1_788_000_000_500);
    expect(timestampMs(undefined)).toBeNull();
  });

  it('formats a date as YYYY-MM-DD in UTC, so the server and the client never disagree', () => {
    expect(formatDate(NOW)).toBe('2026-09-18');
    expect(formatDate(null)).toBeNull();
  });
});

describe('the labels', () => {
  it('names each key status', () => {
    expect(KEY_STATUS_LABEL).toEqual({ active: 'Active', revoked: 'Revoked', expired: 'Expired', inactive: 'Inactive (account archived)' });
  });

  it('offers exactly the four expiry choices of D7, with the default label of § 5.4', () => {
    expect([...EXPIRY_CHOICES]).toEqual(['default', '30', '90', '365']);
    expect(EXPIRY_LABEL.default).toBe('IAM default (the key may not expire)');
  });
});
