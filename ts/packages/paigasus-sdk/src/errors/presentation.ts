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
// demanding an entry for it would make the table 100 keys rather than 99.
import { ErrorReason } from '@paigasus/proto';

import type { Presentation } from './types';

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
  [ErrorReason.PRINCIPAL_INACTIVE]: 'disabled',
  [ErrorReason.AUTHN_UNAVAILABLE]: 'from-transport',
  [ErrorReason.GRANTS_SURVIVE]: 'from-transport',
  [ErrorReason.DECISION_CHANGE_UNACKNOWLEDGED]: 'from-transport',
  [ErrorReason.INVALID_TIMESTAMP]: 'from-transport',
  [ErrorReason.INVALID_UUID]: 'from-transport',
  [ErrorReason.INVALID_CURSOR]: 'from-transport',
  [ErrorReason.INVALID_AUDIT_OUTCOME]: 'from-transport',
  [ErrorReason.INVALID_PRINCIPAL_KIND]: 'from-transport',
  [ErrorReason.MISSING_REQUIRED_FIELD]: 'from-transport',
  [ErrorReason.MUTUALLY_EXCLUSIVE_FIELDS]: 'from-transport',
  [ErrorReason.SERVICE_MIGRATING]: 'from-transport',
  // SMA-712. The operator identity-link calls. The transport status table already presents
  // 400 as invalid input and 409 as a conflict.
  [ErrorReason.INVALID_REASON]: 'from-transport',
  [ErrorReason.UNKNOWN_ISSUER]: 'from-transport',
  [ErrorReason.INVALID_SUBJECT]: 'from-transport',
  [ErrorReason.CANNOT_UNLINK_OWN_IDENTITY]: 'from-transport',
  [ErrorReason.EXTERNAL_IDENTITY_EXISTS]: 'from-transport',
  // SMA-700. IAM's own DPoP quota refusal (gRPC RESOURCE_EXHAUSTED with RetryInfo). The transport
  // table already presents it as rate-limited ("try again soon").
  [ErrorReason.DPOP_QUOTA_EXCEEDED]: 'from-transport',
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
  // SMA-635. Both answer 400, which the transport table already presents as invalid input, as it
  // does for the other gateway 400 codes.
  [ErrorReason.INVALID_ORG_HEADER]: 'from-transport',
  [ErrorReason.ORG_REQUIRED]: 'from-transport',
  // SMA-677. The gateway's own 429s. A rate refusal keeps the transport table's `rate-limited`
  // answer ("try again soon"). A used-up budget cannot succeed for hours, so it gets its own
  // presentation, whose copy says to ask an administrator instead of to try again.
  [ErrorReason.RATE_LIMITED]: 'from-transport',
  [ErrorReason.BUDGET_EXHAUSTED]: 'quota-exhausted',
  // SMA-749. The gateway could not check a non-stream answer for tool calls, so it refused to
  // forward it. A 502 like an upstream failure: the transport table's `degraded` copy fits.
  [ErrorReason.TOOL_INTERCEPTION_MALFORMED_RESPONSE]: 'from-transport',
  // SMA-769.
  [ErrorReason.RESPONSES_STATEFUL_NOT_SUPPORTED]: 'from-transport',
  [ErrorReason.INTERNAL]: 'from-transport',
  [ErrorReason.INVALID_REQUEST_BODY]: 'from-transport',
  [ErrorReason.REQUEST_TOO_LARGE]: 'from-transport',
  [ErrorReason.MISSING_AUTH_CONTEXT]: 'from-transport',
  [ErrorReason.CAPABILITY_DISABLED]: 'disabled',
  [ErrorReason.UNSUPPORTED_CONTENT_TYPE]: 'from-transport',
  [ErrorReason.INVALID_REQUEST_SCHEMA]: 'invalid-input',
  [ErrorReason.INVALID_QUERY_PARAMETER]: 'from-transport',
  [ErrorReason.INVALID_PATH_SEGMENT]: 'from-transport',
  // SMA-700. A 401 from both services; the transport table already presents it as relogin.
  [ErrorReason.INVALID_DPOP_PROOF]: 'from-transport',
  // SMA-787. A 503 from the agent host. The transport status table already presents it.
  [ErrorReason.STORE_UNAVAILABLE]: 'from-transport',
  // SMA-852. The agent registry reasons. The transport status table already presents each status.
  [ErrorReason.AGENT_NAME_TAKEN]: 'from-transport',
  [ErrorReason.INBOX_EXISTS]: 'from-transport',
  [ErrorReason.AGENT_VERSION_MISMATCH]: 'from-transport',
  [ErrorReason.AGENT_ARCHIVED]: 'from-transport',
  [ErrorReason.UNKNOWN_MODEL]: 'from-transport',
  [ErrorReason.UNKNOWN_TOOL]: 'from-transport',
  [ErrorReason.INVALID_AGENT_FIELD]: 'from-transport',
  // SMA-774. The IAM tool catalog and tool policy set calls. The transport status table already
  // presents each of their statuses (invalid input, conflict, not found, unavailable).
  [ErrorReason.TOOL_POLICY_INVALID]: 'from-transport',
  [ErrorReason.TOOL_POLICY_SHAPE_UNSUPPORTED]: 'from-transport',
  [ErrorReason.TOOL_POLICY_LOWERING_FAILED]: 'from-transport',
  [ErrorReason.TOOL_POLICY_FLOOR_PERMIT]: 'from-transport',
  [ErrorReason.TOOL_POLICY_ID_INVALID]: 'from-transport',
  [ErrorReason.TOOL_POLICY_WINDOW_TOO_LONG]: 'from-transport',
  [ErrorReason.TOOL_CATALOG_BREAKS_POLICY]: 'from-transport',
  [ErrorReason.TOOL_CATALOG_INVALID]: 'from-transport',
  [ErrorReason.TOOL_CATALOG_CONFLICT]: 'from-transport',
  [ErrorReason.TOOL_POLICY_VERSION_CONFLICT]: 'from-transport',
  [ErrorReason.TOOL_POLICY_RECOMPILE_PENDING]: 'from-transport',
  [ErrorReason.TOOL_POLICY_COMPILER_UNAVAILABLE]: 'from-transport',
  [ErrorReason.TOOL_POLICY_COMPILER_BUSY]: 'from-transport',
  [ErrorReason.TOOL_POLICY_VERSION_NOT_FOUND]: 'from-transport',
  [ErrorReason.TOOL_POLICY_MODE_TRANSITION_INVALID]: 'from-transport',
  [ErrorReason.TOOL_POLICY_HISTORY_UNSUPPORTED]: 'from-transport',
  [ErrorReason.TOOL_POLICY_HELD_NOT_ACCEPTABLE]: 'from-transport',
  // SMA-752. The tool-policy reasons appear in decision reports and notices, never as an HTTP
  // error (D11), so the transport table's answer applies if one ever reaches a console.
  [ErrorReason.TOOL_CALL_DENIED]: 'from-transport',
  [ErrorReason.TOOL_RESULT_BLOCKED]: 'from-transport',
  [ErrorReason.TOOL_POLICY_UNAVAILABLE]: 'from-transport',
  // SMA-776. The only tool-decision reason that is an HTTP error: the gateway refuses a request
  // with a blocked computer_call_output. A 400, so the transport table's answer applies.
  [ErrorReason.TOOL_RESULT_UNREPLACEABLE]: 'from-transport',
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
