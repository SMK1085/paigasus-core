// SPDX-License-Identifier: Apache-2.0
//
// The view model and the copy of the "Model access for people" section (SMA-676 spec § 4.4, § 5).
// PLAIN DATA ONLY: a server loader builds it and a client component receives it. No `server-only`
// import and no runtime import except the copy below; the type imports are erased.
import type { FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

export type HolderRowView = {
  readonly grantId: string;
  readonly principalPrn: string;
  /** D13: false shows "Not a member"; null hides the mark because the member list is truncated. */
  readonly member: boolean | null;
};

export type CandidateView = { readonly principalPrn: string };

export type PeopleFlags = { readonly canGrant: boolean; readonly canRevoke: boolean; readonly grantsTruncated: boolean; readonly membersTruncated: boolean };

export type PeopleModelAccessOk = {
  readonly kind: 'ok';
  readonly orgPrn: string;
  /** § 5.1 step 4: the org or an ancestor is not active, so IAM refuses changes. */
  readonly readOnly: boolean;
  readonly holders: readonly HolderRowView[];
  readonly candidates: readonly CandidateView[];
  readonly flags: PeopleFlags;
};

export type PeopleModelAccessView = { readonly kind: 'disabled' } | { readonly kind: 'denied' } | { readonly kind: 'error'; readonly error: PaigasusError } | PeopleModelAccessOk;

export type PeopleModelAccessActions = { readonly grant: FormAction; readonly revoke: FormAction };

export const PEOPLE_COPY = {
  title: 'Model access for people',
  disabled: 'Role administration is not enabled on this IAM.',
  denied: 'You cannot see model access for this organization.',
  readOnly: 'This organization is not active. IAM refuses changes to its role grants.',
  noHolders: 'No person holds model access.',
  noCandidates: 'No other person to grant model access to.',
  notMember: 'Not a member',
  grantsTruncated: 'Showing the first 1000 role grants at this organization.',
  membersTruncated: 'Showing the first 1000 members of this organization.',
  choose: 'Choose a person',
  grant: 'Grant model access',
  revoke: 'Revoke',
} as const;
