// SPDX-License-Identifier: Apache-2.0
//
// The user-facing words for every error (spec § 6.5, D16; SMA-636 § 5.1). CLIENT-SAFE: no
// `server-only`, and the only import is @paigasus/sdk's guard-free ./errors/types entry
// (ts/packages/paigasus-sdk/src/errors/types.ts), so form-error.tsx (a client component) can use it.
//
// Every error this zone shows comes from a call to IAM, so the tables are IAM-worded throughout,
// same as iam-console's. `iam-console` keeps its OWN copy rather than sharing one (plan D16): the
// two zones are separate products whose wording may legitimately differ, and
// @paigasus/console-core's boundary rule bans a React component from its `src/`.
//
// Two tables. PRESENTATION_COPY is total over Presentation, so a tenth presentation fails the
// type-check. FORM_REASON_COPY covers the reasons the service-account forms can get (SMA-636); a
// test asserts every key is a real ErrorReason. A reason with no entry — including one this build
// does not know, which the SDK reports as reason null — falls back to the presentation's copy (the
// version-skew rule). No copy ever repeats IAM's message.
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';

export const PRESENTATION_COPY: Record<Presentation, { title: string; body: string }> = {
  relogin: { title: 'Your session has ended', body: 'Sign in again to continue.' },
  forbidden: { title: 'You do not have access', body: 'IAM refused this request for your account.' },
  'not-found': { title: 'Not found', body: 'This item does not exist, or it was removed.' },
  degraded: { title: 'IAM is not available', body: 'IAM did not answer in time. Try again in a moment.' },
  'rate-limited': { title: 'Too many requests', body: 'Wait a moment, then try again.' },
  'invalid-input': { title: 'The request was not valid', body: 'Check the values and try again.' },
  conflict: { title: 'The request conflicts with the current state', body: 'Reload the page and try again.' },
  disabled: { title: 'This feature is not enabled on this IAM', body: 'An operator can enable it in the IAM configuration.' },
  generic: { title: 'Something went wrong', body: 'The request failed. Try again, and give the reference below to support if it fails again.' },
};

export const FORM_REASON_COPY: Partial<Record<ErrorReason, string>> = {
  // SMA-636 § 5.2.
  [ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT]: 'A service account with this name already exists here.',
  [ErrorReason.INVALID_NAME]: 'Enter a name.',
  [ErrorReason.NOT_FOUND]: 'The item was not found. It may have been removed.',
  [ErrorReason.MISSING_REQUIRED_FIELD]: 'Fill in every required field.',
  [ErrorReason.FORBIDDEN]: 'You do not have permission to do this.',
  [ErrorReason.PARENT_ARCHIVED]: 'The parent of this item is archived.',
  [ErrorReason.NODE_ARCHIVED]: 'This item is archived.',
  // IAM answers UNIMPLEMENTED with this reason when a capability is off (spec § 3.3).
  [ErrorReason.CAPABILITY_DISABLED]: 'This feature is not enabled on this IAM.',
  [ErrorReason.PRINCIPAL_INACTIVE]: 'Your account is not active in IAM.',
};

/** The one sentence a form shows for an error. */
export function formMessage(error: PaigasusError): string {
  const byReason = error.reason === null ? undefined : FORM_REASON_COPY[error.reason];
  return byReason ?? PRESENTATION_COPY[error.presentation].body;
}
