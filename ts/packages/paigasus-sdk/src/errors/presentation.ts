// SPDX-License-Identifier: Apache-2.0
//
// The reason -> presentation OVERRIDE table (spec § 9.4). Internal: reached through ./errors.
//
// Two mechanisms guard it and BOTH are required. This total `Record` makes a missing reason a
// COMPILE error — MEASURED on tsc 6.0.3, omitting a member yields "TS2741: Property
// '[ErrorReason.INTERNAL]' is missing". `tests/presentation.test.ts` iterates
// ErrorReasonSchema.values and makes it a TEST failure. The type alone can be switched off by a
// refactor to `Partial<Record<…>>`, which the test notices; the test alone runs later.
//
// The `Exclude` is load-bearing: UNSPECIFIED is the zero sentinel, the test skips it, and
// demanding an entry for it would make the table 58 keys rather than 57.
import { ErrorReason } from '@paigasus/proto';

import type { Presentation } from './types.js';

/** `'from-transport'` means "take the transport status table's answer", not "no opinion". */
export type PresentationEntry = Presentation | 'from-transport';

export const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, PresentationEntry> = {
  [ErrorReason.SLUG_CONFLICT]: 'from-transport',
  [ErrorReason.DUPLICATE_MEMBERSHIP]: 'from-transport',
  [ErrorReason.EMAIL_CONFLICT]: 'from-transport',
  [ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT]: 'from-transport',
  [ErrorReason.INVALID_EMAIL]: 'from-transport',
  [ErrorReason.INVALID_SLUG]: 'from-transport',
  [ErrorReason.INVALID_NAME]: 'from-transport',
  [ErrorReason.INVALID_PRN]: 'from-transport',
  [ErrorReason.PRN_MISMATCH]: 'from-transport',
  [ErrorReason.INVALID_PAGINATION]: 'from-transport',
  [ErrorReason.NOTHING_TO_RENAME]: 'from-transport',
  [ErrorReason.NOT_FOUND]: 'from-transport',
  [ErrorReason.PARENT_ARCHIVED]: 'from-transport',
  [ErrorReason.NODE_ARCHIVED]: 'from-transport',
  [ErrorReason.MISSING_ORG_MEMBERSHIP]: 'from-transport',
  [ErrorReason.FORBIDDEN]: 'from-transport',
  [ErrorReason.UNKNOWN_ROLE]: 'from-transport',
  [ErrorReason.INVALID_SCOPE]: 'from-transport',
  [ErrorReason.SYSTEM_IMMUTABLE]: 'from-transport',
  [ErrorReason.POLICY_INVALID]: 'from-transport',
  [ErrorReason.POLICY_CONFLICT]: 'from-transport',
  [ErrorReason.INVALID_ACTION]: 'from-transport',
  [ErrorReason.INVALID_BULK_REPLAY]: 'from-transport',
  [ErrorReason.NOT_SYSTEM_OWNED]: 'from-transport',
  [ErrorReason.FLEET_NOT_CONVERGED]: 'from-transport',
  [ErrorReason.INVALID_TOKEN]: 'from-transport',
  [ErrorReason.IDENTITY_NOT_PROVISIONED]: 'from-transport',
  [ErrorReason.PROVISIONING_FAILED]: 'from-transport',
  [ErrorReason.PRINCIPAL_INACTIVE]: 'from-transport',
  [ErrorReason.AUTHN_UNAVAILABLE]: 'from-transport',
  [ErrorReason.GRANTS_SURVIVE]: 'from-transport',
  [ErrorReason.DECISION_CHANGE_UNACKNOWLEDGED]: 'from-transport',
  [ErrorReason.INVALID_TIMESTAMP]: 'from-transport',
  [ErrorReason.INVALID_UUID]: 'from-transport',
  [ErrorReason.INVALID_CURSOR]: 'from-transport',
  [ErrorReason.INVALID_AUDIT_OUTCOME]: 'from-transport',
  [ErrorReason.MISSING_REQUIRED_FIELD]: 'from-transport',
  [ErrorReason.MUTUALLY_EXCLUSIVE_FIELDS]: 'from-transport',
  [ErrorReason.SERVICE_MIGRATING]: 'from-transport',
  [ErrorReason.MISSING_AUTHORIZATION]: 'from-transport',
  [ErrorReason.INVALID_API_KEY]: 'from-transport',
  [ErrorReason.INSUFFICIENT_PERMISSIONS]: 'from-transport',
  [ErrorReason.MISSING_SCOPE]: 'from-transport',
  [ErrorReason.IAM_UNAVAILABLE]: 'from-transport',
  // ---- overrides (spec § 9.4) ----
  [ErrorReason.UPSTREAM_UNAVAILABLE]: 'degraded',
  [ErrorReason.UPSTREAM_TIMEOUT]: 'degraded',
  [ErrorReason.UPSTREAM_ERROR]: 'degraded',
  [ErrorReason.STREAMING_DISABLED]: 'from-transport',
  [ErrorReason.INTERNAL]: 'from-transport',
  [ErrorReason.INVALID_REQUEST_BODY]: 'from-transport',
  [ErrorReason.REQUEST_TOO_LARGE]: 'from-transport',
  [ErrorReason.MISSING_AUTH_CONTEXT]: 'from-transport',
  [ErrorReason.CAPABILITY_DISABLED]: 'disabled',
  [ErrorReason.UNSUPPORTED_CONTENT_TYPE]: 'from-transport',
  [ErrorReason.INVALID_REQUEST_SCHEMA]: 'invalid-input',
  [ErrorReason.INVALID_QUERY_PARAMETER]: 'from-transport',
  [ErrorReason.INVALID_PATH_SEGMENT]: 'from-transport',
};

/**
 * The presentation this reason overrides the transport status with, or `null` to defer.
 *
 * Returns `null` for an unmapped reason and for the zero sentinel, so a caller can always fall
 * back to the transport table without a special case.
 */
export function presentationOverride(reason: ErrorReason | null): Presentation | null {
  if (reason === null || reason === ErrorReason.UNSPECIFIED) return null;
  const entry = PRESENTATION[reason] as PresentationEntry | undefined;
  if (entry === undefined || entry === 'from-transport') return null;
  return entry;
}
