// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section of the organization and project pages (SMA-636 spec § 4.2). It
// returns a VIEW MODEL (./view): plain data that a client component can receive, never a proto
// message. All IAM calls go through callIam.
//
// The fan-out is bounded (D14): the list and the five affordances always; the account, its
// model-call state and its keys only for the ONE account that `?sa=` selects.
//
// Keys load for an archived account too (plan SPEC DEVIATION 4): § 5.7 shows them as "Inactive
// (account archived)". modelCallState makes no call for such an account (§ 4.3).
import 'server-only';
import type { ServiceState } from '@paigasus/discovery/types';
import { callIam, cedarCapabilityOf, type IamClients, type MayI } from '@paigasus/console-core';
import { REQUEST_LIMIT, pageOf } from '../../../lib/paging';
import { lifecycleView, type NodeLifecycle } from '../node-status';
import { formatDate, keyStatus, timestampMs } from './keys';
import { modelCallState } from './model-call-state';
import { serviceAccountIdOf, serviceAccountPrn } from './service-account-id';
import type { ApiKeyRowView, KeysView, ReadOnlyView, SectionView, SelectedView, ServiceAccountRowView } from './view';

/** The IAM capability that gates every key control and every ListApiKeys call (D13). */
export const API_KEYS_CAPABILITY = 'iam.apikeys';

/** D13: the rule of cedarCapabilityOf, for iam.apikeys. A degraded IAM counts as absent. */
export function apiKeysCapabilityOf(state: ServiceState): boolean {
  return state.state === 'available' && state.capabilities.includes(API_KEYS_CAPABILITY);
}

type ServiceAccounts = IamClients['serviceAccounts'];
type AccountMessage = Awaited<ReturnType<ServiceAccounts['listServiceAccounts']>>['serviceAccounts'][number];
type KeyMessage = Awaited<ReturnType<ServiceAccounts['listApiKeys']>>['apiKeys'][number];

export type SectionDeps = {
  readonly serviceAccounts: Pick<ServiceAccounts, 'listServiceAccounts' | 'getServiceAccount' | 'listApiKeys'>;
  readonly authz: Pick<IamClients['authz'], 'isAuthorized'>;
  readonly mayI: MayI;
  /** This request's IAM discovery state (memoized per request by discovery()). */
  readonly iam: ServiceState;
  readonly now: () => number;
};

export type SectionParams = {
  readonly ownerPrn: string;
  readonly lifecycle: NodeLifecycle;
  readonly saOffset: number;
  readonly keyOffset: number;
  /** The UUID `?sa=` selects, or null. */
  readonly sa: string | null;
};

type Affordances = { readonly issue: boolean; readonly revoke: boolean; readonly archive: boolean; readonly grant: boolean };
type Flags = { readonly writable: boolean; readonly apiKeys: boolean; readonly cedar: boolean; readonly can: Affordances };

function accountRow(account: AccountMessage): ServiceAccountRowView {
  return { prn: account.prn, id: serviceAccountIdOf(account.prn), name: account.name, created: formatDate(timestampMs(account.audit?.createdAt)), active: account.status === 'active' };
}

/** § 5.8: read-only unless the owner node itself and every ancestor are active. */
function readOnlyOf(lifecycle: NodeLifecycle): ReadOnlyView {
  const view = lifecycleView(lifecycle);
  return view === 'active' ? null : view;
}

function keyRow(key: KeyMessage, ownerPrn: string, accountActive: boolean, now: number): ApiKeyRowView {
  const expiresAtMs = timestampMs(key.expiresAt);
  return {
    id: key.id,
    prefix: key.prefix,
    status: keyStatus({ status: key.status, expiresAtMs }, accountActive, now),
    created: formatDate(timestampMs(key.audit?.createdAt)),
    expires: formatDate(expiresAtMs),
    lastUsed: formatDate(timestampMs(key.lastUsedAt)),
    otherScope: key.scopePrn === ownerPrn ? null : key.scopePrn,
  };
}

async function loadKeys(deps: SectionDeps, saPrn: string, params: SectionParams, accountActive: boolean): Promise<KeysView> {
  const listed = await callIam(() => deps.serviceAccounts.listApiKeys({ serviceAccountPrn: saPrn, limit: REQUEST_LIMIT, offset: BigInt(params.keyOffset) }));
  if (!listed.ok) return { kind: 'error', error: listed.error };
  const now = deps.now();
  const page = pageOf(
    listed.value.apiKeys.map((key) => keyRow(key, params.ownerPrn, accountActive, now)),
    params.keyOffset,
  );
  return { kind: 'ok', rows: page.rows, page: { offset: page.offset, nextOffset: page.nextOffset } };
}

async function loadSelected(deps: SectionDeps, params: SectionParams, sa: string, flags: Flags): Promise<SelectedView> {
  const got = await callIam(() => deps.serviceAccounts.getServiceAccount({ prn: serviceAccountPrn(sa) }));
  if (!got.ok) return { kind: 'error', error: got.error };
  const account = got.value.serviceAccount;
  // Another owner's account is never shown here, not even its name (§ 4.2 step 4). An answer with
  // no account (version skew) is treated the same way: nothing about it is shown.
  if (account === undefined || account.ownerPrn !== params.ownerPrn) return { kind: 'other-scope' };
  const row = accountRow(account);
  const [modelCalls, keys] = await Promise.all([
    modelCallState(deps.authz, account.prn, params.ownerPrn, account.status),
    flags.apiKeys ? loadKeys(deps, account.prn, params, row.active) : Promise.resolve<KeysView>({ kind: 'hidden' }),
  ]);
  // An archived account has no controls (§ 5.6); an owner that is not active has none (§ 5.8).
  const live = flags.writable && row.active;
  return {
    kind: 'ok',
    account: row,
    modelCalls,
    keys,
    controls: {
      // § 5.3: all four conditions.
      allow: live && modelCalls === 'no' && flags.cedar && flags.can.grant,
      issue: live && flags.apiKeys && flags.can.issue,
      revoke: live && flags.apiKeys && flags.can.revoke,
      archive: live && flags.can.archive,
    },
  };
}

export async function loadServiceAccountSection(deps: SectionDeps, params: SectionParams): Promise<SectionView> {
  const owner = params.ownerPrn;
  const [listed, canCreate, canIssue, canRevoke, canArchive, canGrant] = await Promise.all([
    callIam(() => deps.serviceAccounts.listServiceAccounts({ ownerPrn: owner, limit: REQUEST_LIMIT, offset: BigInt(params.saOffset) })),
    deps.mayI('CreateServiceAccount', owner),
    deps.mayI('IssueApiKey', owner),
    deps.mayI('RevokeApiKey', owner),
    deps.mayI('ArchiveServiceAccount', owner),
    deps.mayI('GrantRole', owner),
  ]);
  if (!listed.ok) return listed.error.presentation === 'forbidden' ? { kind: 'denied' } : { kind: 'error', error: listed.error };

  const readOnly = readOnlyOf(params.lifecycle);
  const flags: Flags = {
    writable: readOnly === null,
    apiKeys: apiKeysCapabilityOf(deps.iam),
    cedar: cedarCapabilityOf(deps.iam),
    can: { issue: canIssue, revoke: canRevoke, archive: canArchive, grant: canGrant },
  };
  const page = pageOf(listed.value.serviceAccounts.map(accountRow), params.saOffset);
  const selected: SelectedView = params.sa === null ? { kind: 'none' } : await loadSelected(deps, params, params.sa, flags);
  return {
    kind: 'ok',
    ownerPrn: owner,
    readOnly,
    rows: page.rows,
    page: { offset: page.offset, nextOffset: page.nextOffset },
    canCreate: flags.writable && canCreate,
    selected,
    sa: params.sa,
    saOffset: params.saOffset,
    keyOffset: params.keyOffset,
  };
}
