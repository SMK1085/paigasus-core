// SPDX-License-Identifier: Apache-2.0
//
// The view model of the service-accounts section (SMA-636 spec § 4.2, § 4.6), the result types of
// its actions (§ 5.2, § 5.4) and their labels. PLAIN DATA ONLY: a server loader builds it and a
// client component receives it, so it never holds a proto message. No `server-only` import and no
// runtime import: a client component may import it. The type imports are erased.
import type { ActionState, FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';

/** Spec § 4.3. */
export type ModelCallState = 'yes' | 'no' | 'archived' | 'unknown';

/** Spec § 5.7. */
export type KeyStatus = 'active' | 'revoked' | 'expired' | 'inactive';

export const KEY_STATUS_LABEL: Readonly<Record<KeyStatus, string>> = {
  active: 'Active',
  revoked: 'Revoked',
  expired: 'Expired',
  inactive: 'Inactive (account archived)',
};

/** D7: the IAM default or 30, 90 or 365 days. The form sends the choice; the server computes the date. */
export const EXPIRY_CHOICES = ['default', '30', '90', '365'] as const;
export type ExpiryChoice = (typeof EXPIRY_CHOICES)[number];

export const EXPIRY_LABEL: Readonly<Record<ExpiryChoice, string>> = {
  default: 'IAM default (the key may not expire)',
  '30': '30 days',
  '90': '90 days',
  '365': '365 days',
};

/** § 5.2. `granted: false` means `iam.authz.cedar` is absent. */
export type CreateState =
  | { readonly kind: 'created'; readonly saPrn: string; readonly granted: boolean }
  | { readonly kind: 'partial'; readonly saPrn: string; readonly error: PaigasusError }
  | { readonly kind: 'failed'; readonly error: PaigasusError }
  | null;

/** § 5.4. The token is in this value only (rule 1). */
export type IssueKeyState = { readonly ok: true; readonly token: string; readonly prefix: string } | { readonly ok: false; readonly error: PaigasusError } | null;

export type PageLinks = { readonly offset: number; readonly nextOffset: number | null };

export type ServiceAccountRowView = {
  readonly prn: string;
  /** The UUID of the PRN, or null when the PRN is not a principal PRN (then the row has no Select link). */
  readonly id: string | null;
  readonly name: string;
  /** YYYY-MM-DD, or null. */
  readonly created: string | null;
  readonly active: boolean;
};

export type ApiKeyRowView = {
  readonly id: string;
  readonly prefix: string;
  readonly status: KeyStatus;
  readonly created: string | null;
  /** Null: the key never expires. */
  readonly expires: string | null;
  /** Null: never used. */
  readonly lastUsed: string | null;
  /** The key's scope when it is NOT the owner node (a key made outside the console), else null. */
  readonly otherScope: string | null;
};

export type KeysView =
  { readonly kind: 'hidden' } | { readonly kind: 'error'; readonly error: PaigasusError } | { readonly kind: 'ok'; readonly rows: readonly ApiKeyRowView[]; readonly page: PageLinks };

/** Which panel controls show. The loader already combined mayI(), the capabilities and both lifecycles. */
export type PanelControls = { readonly allow: boolean; readonly issue: boolean; readonly revoke: boolean; readonly archive: boolean };

export type SelectedView =
  | { readonly kind: 'none' }
  | { readonly kind: 'error'; readonly error: PaigasusError }
  | { readonly kind: 'other-scope' }
  | { readonly kind: 'ok'; readonly account: ServiceAccountRowView; readonly modelCalls: ModelCallState; readonly keys: KeysView; readonly controls: PanelControls };

/** § 5.8. Null: the owner node is active and the section is writable. */
export type ReadOnlyView = 'archived' | 'archived-parent' | 'unknown' | null;

export type SectionOk = {
  readonly kind: 'ok';
  readonly ownerPrn: string;
  readonly readOnly: ReadOnlyView;
  readonly rows: readonly ServiceAccountRowView[];
  readonly page: PageLinks;
  readonly canCreate: boolean;
  readonly selected: SelectedView;
  /** The `sa` search parameter, kept in the pager links. */
  readonly sa: string | null;
  readonly saOffset: number;
  readonly keyOffset: number;
};

export type SectionView = { readonly kind: 'denied' } | { readonly kind: 'error'; readonly error: PaigasusError } | SectionOk;

export type OwnerKind = 'organization' | 'project';

export type CreateAction = (previous: CreateState, form: FormData) => Promise<CreateState>;

/** § 5.4 rule 2: the client calls it DIRECTLY, with `null` as the previous state. */
export type IssueKeyAction = (previous: null, form: FormData) => Promise<IssueKeyState>;

export type ServiceAccountActions = {
  readonly create: CreateAction;
  readonly allow: FormAction;
  readonly issue: IssueKeyAction;
  readonly revoke: FormAction;
  readonly archive: FormAction;
};

export type SimpleControl = 'allow' | 'revoke' | 'archive' | 'issue';

/**
 * The ONE result region of a section (§ 4.6): the last result of any of its actions.
 *
 * The `issue` arm holds a FAILURE only. A success carries the token, and § 6.1 allows the token in
 * one browser place, TokenPanel's own useState; storing it here would make a second copy. So an
 * IssueKeyState success is not assignable to this type (tests/unit/section-result-type.test.ts).
 * `unreached` is a rejected action (a client-built error). `token-lost` is a minted key whose token
 * could not reach the TokenPanel: it holds the key prefix, never the token.
 */
export type SectionResult =
  | { readonly control: 'create'; readonly state: Exclude<CreateState, null> }
  | { readonly control: Exclude<SimpleControl, 'issue'>; readonly state: Exclude<ActionState, null> }
  | { readonly control: 'issue'; readonly state: { readonly ok: false; readonly error: PaigasusError } }
  | { readonly control: 'unreached'; readonly error: PaigasusError }
  | { readonly control: 'token-lost'; readonly prefix: string }
  | null;
