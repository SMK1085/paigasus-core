// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { ErrorReason } from '@paigasus/proto';

import type { Presentation } from './types.js';

export type PresentationEntry = Presentation | 'from-transport';

/**
 * reason -> presentation, for the reasons whose product meaning differs from what their transport
 * status alone conveys (spec § 9.4).
 *
 * TOTAL over the 57 real reasons by TYPE. Omitting a member is `TS2741`, and the error NAMES the
 * missing one (spec M8) — which is how this table was written and how it must be extended. The
 * type alone is not enough, because a refactor to `Partial<...>` would switch it off silently;
 * `tests/errors-presentation.test.ts` iterates the descriptor and notices. Both are kept.
 *
 * `'from-transport'` is not a default that happened — it is a reviewed decision recorded for each
 * of the 54 reasons that take it.
 */
const PRESENTATION: Record<Exclude<ErrorReason, ErrorReason.UNSPECIFIED>, PresentationEntry> = {
  // ---- IAM (1-39) -----------------------------------------------------------
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
  // "identity-not-provisioned" — a provisioning state an admin resolves. Kept on the transport
  // deliberately: its neighbour below is lifted, this one is not (spec § 9.4).
  [ErrorReason.IDENTITY_NOT_PROVISIONED]: 'from-transport',
  // "provisioning-failed" — the other provisioning-state neighbour, also left on the transport.
  [ErrorReason.PROVISIONING_FAILED]: 'from-transport',
  // PermissionDenied/403 on the wire, but the account exists and is deactivated — a different
  // screen from "you do not have permission".
  [ErrorReason.PRINCIPAL_INACTIVE]: 'disabled',
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

  // ---- Gateway (300-308) -----------------------------------------------------
  [ErrorReason.MISSING_AUTHORIZATION]: 'from-transport',
  [ErrorReason.INVALID_API_KEY]: 'from-transport',
  [ErrorReason.INSUFFICIENT_PERMISSIONS]: 'from-transport',
  [ErrorReason.MISSING_SCOPE]: 'from-transport',
  [ErrorReason.IAM_UNAVAILABLE]: 'from-transport',
  [ErrorReason.UPSTREAM_UNAVAILABLE]: 'from-transport',
  [ErrorReason.UPSTREAM_TIMEOUT]: 'from-transport',
  [ErrorReason.UPSTREAM_ERROR]: 'from-transport',
  [ErrorReason.STREAMING_DISABLED]: 'from-transport',

  // ---- Shared (900-908) -------------------------------------------------------
  [ErrorReason.INTERNAL]: 'from-transport',
  [ErrorReason.INVALID_REQUEST_BODY]: 'from-transport',
  [ErrorReason.REQUEST_TOO_LARGE]: 'from-transport',
  [ErrorReason.MISSING_AUTH_CONTEXT]: 'from-transport',
  // Reaches a client only as gRPC Unimplemented, which § 9.2 maps to `generic`. The capability
  // name rides in metadata["capability"], so the screen can name what is switched off.
  [ErrorReason.CAPABILITY_DISABLED]: 'disabled',
  [ErrorReason.UNSUPPORTED_CONTENT_TYPE]: 'from-transport',
  // A PIN. IAM answers 422 and the gateway 400 for this one code; both already resolve to
  // invalid-input, and this entry stops a future status change from splitting them.
  [ErrorReason.INVALID_REQUEST_SCHEMA]: 'invalid-input',
  [ErrorReason.INVALID_QUERY_PARAMETER]: 'from-transport',
  [ErrorReason.INVALID_PATH_SEGMENT]: 'from-transport',
};

/**
 * The lookup, as a function rather than a bare index.
 *
 * `fromWireReason` returns `ErrorReason | undefined` — a type that INCLUDES UNSPECIFIED even
 * though the implementation excludes it at runtime — and no narrowing expresses "not UNSPECIFIED".
 * Indexing PRESENTATION directly at a call site therefore does not compile. This is the one place
 * that handles the sentinel (spec § 9.4).
 */
export function presentationFor(reason: ErrorReason): PresentationEntry {
  return reason === ErrorReason.UNSPECIFIED ? 'from-transport' : PRESENTATION[reason];
}
