// SPDX-License-Identifier: Apache-2.0
//
// API key facts as the screens show them (SMA-636 spec § 5.4, § 5.7). Pure, with an injected
// clock. The loader and the issue command use it on the server. Its only runtime import is
// @paigasus/sdk's guard-free ./iam/types entry.
import { ApiKeyStatus } from '@paigasus/sdk/iam/types';
import type { ExpiryChoice, KeyStatus } from './view';

const DAY_MS = 86_400_000;

const EXPIRY_DAYS: Readonly<Record<Exclude<ExpiryChoice, 'default'>, number>> = { '30': 30, '90': 90, '365': 365 };

/** A protobuf Timestamp as milliseconds, or null when it is not set. */
export function timestampMs(value: { readonly seconds: bigint; readonly nanos: number } | undefined): number | null {
  if (value === undefined) return null;
  return Number(value.seconds) * 1000 + Math.floor(value.nanos / 1_000_000);
}

/** YYYY-MM-DD in UTC, or null. The server formats every date, so a client render never differs. */
export function formatDate(ms: number | null): string | null {
  return ms === null ? null : new Date(ms).toISOString().slice(0, 10);
}

/** § 5.7, in this order: account not active, revoked, expired, else active. */
export function keyStatus(key: { readonly status: ApiKeyStatus; readonly expiresAtMs: number | null }, accountActive: boolean, nowMs: number): KeyStatus {
  if (!accountActive) return 'inactive';
  if (key.status === ApiKeyStatus.REVOKED) return 'revoked';
  if (key.expiresAtMs !== null && key.expiresAtMs <= nowMs) return 'expired';
  return 'active';
}

/**
 * The `expires_at` of IssueApiKey (§ 5.4). Unset for 'default': IAM then applies its
 * `default_expiry_days`, and with none set the key never expires. Else the clock plus N days, in
 * whole seconds. IAM enforces no maximum, so the console invents none (D7).
 */
export function expiresAtFor(choice: ExpiryChoice, nowMs: number): { seconds: bigint; nanos: number } | undefined {
  if (choice === 'default') return undefined;
  return { seconds: BigInt(Math.floor((nowMs + EXPIRY_DAYS[choice] * DAY_MS) / 1000)), nanos: 0 };
}
