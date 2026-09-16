// SPDX-License-Identifier: Apache-2.0
//
// The user-facing words for every error (spec § 6.5, D16). CLIENT-SAFE: no `server-only`, and the
// only import is @paigasus/sdk's guard-free ./errors/types entry
// (ts/packages/paigasus-sdk/src/errors/types.ts).
//
// Every error this zone shows comes from a call to IAM — the gateway zone has no forms and calls
// no IAM-fronted mutation of its own yet, so this table is IAM-worded throughout, same as
// iam-console's. `iam-console` keeps its OWN copy of this table rather than sharing one (plan
// D16): the two zones are separate products whose wording may legitimately differ, and
// @paigasus/console-core's boundary rule bans a React component from its `src/`.
//
// PRESENTATION_COPY is total over Presentation, so a tenth presentation fails the type-check.
import type { Presentation } from '@paigasus/sdk/errors/types';

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
