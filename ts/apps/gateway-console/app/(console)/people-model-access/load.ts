// SPDX-License-Identifier: Apache-2.0
//
// The loader of the "Model access for people" section (SMA-676 spec § 4.4, § 5.1). It returns a
// VIEW MODEL (./view). All IAM calls go through callIam.
//
// D15: with iam.authz.cedar off it makes NO call — ListRoleGrants itself is behind that capability,
// so a call could only produce the error state. Else it reads, in parallel, the USER grants at the
// org and the USER members of the org, each in pages of 200 (IAM's Page maximum; ListMemberships
// refuses more), at most five pages, then one N+1 probe (lib/paging.ts:3-6's rule on a bounded
// read), and asks mayI for GrantRole and RevokeRole. mayI is cosmetic: IAM is the gate (D14).
import 'server-only';
import type { ServiceState } from '@paigasus/discovery/types';
import { PrincipalKind } from '@paigasus/sdk/iam/types';
import { callIam, cedarCapabilityOf, type IamClients, type IamResult, type MayI } from '@paigasus/console-core';
import { GATEWAY_ROLE } from '../gateway-role';
import type { CandidateView, HolderRowView, PeopleModelAccessView } from './view';

/** IAM's Page maximum (application/pagination.rs:11). */
export const LIST_PAGE_SIZE = 200;
/** § 4.4: at most five pages per list. */
export const LIST_MAX_PAGES = 5;
/** § 9: each list stops at 1000 rows. */
export const LIST_ROW_CAP = LIST_PAGE_SIZE * LIST_MAX_PAGES;

export type PeopleModelAccessDeps = {
  readonly authz: Pick<IamClients['authz'], 'listRoleGrants'>;
  readonly tenancy: Pick<IamClients['tenancy'], 'listMemberships'>;
  readonly mayI: MayI;
  /** This request's IAM discovery state (memoized per request by discovery()). */
  readonly iam: ServiceState;
};

export type PeopleModelAccessParams = { readonly orgPrn: string; readonly orgActive: boolean };

type Bounded<T> = { readonly rows: readonly T[]; readonly truncated: boolean };

/** Pages of LIST_PAGE_SIZE until a short page, at most LIST_MAX_PAGES; after five full pages, one probe for row 1001. */
export async function readBounded<T>(read: (limit: number, offset: bigint) => Promise<IamResult<readonly T[]>>): Promise<IamResult<Bounded<T>>> {
  const rows: T[] = [];
  for (let page = 0; page < LIST_MAX_PAGES; page += 1) {
    const got = await read(LIST_PAGE_SIZE, BigInt(page * LIST_PAGE_SIZE));
    if (!got.ok) return got;
    rows.push(...got.value);
    if (got.value.length < LIST_PAGE_SIZE) return { ok: true, value: { rows, truncated: false } };
  }
  const probe = await read(1, BigInt(LIST_ROW_CAP));
  if (!probe.ok) return probe;
  return { ok: true, value: { rows, truncated: probe.value.length > 0 } };
}

type GrantRow = { readonly id: string; readonly principalPrn: string; readonly roleKey: string };
type MemberRow = { readonly principalPrn: string };

/**
 * § 4.4 "Join". Holders = the gateway_user grants. Candidates = (member PRNs ∪ grantee PRNs) − holder
 * PRNs, sorted: a user with ANY role at the org is offered, so the org creator appears (D12). A
 * holder's `member` is null when the member list is truncated (D13).
 */
export function joinPeople(grants: readonly GrantRow[], members: readonly MemberRow[], membersTruncated: boolean): { holders: HolderRowView[]; candidates: CandidateView[] } {
  const memberPrns = new Set(members.map((row) => row.principalPrn));
  const holders = grants
    .filter((row) => row.roleKey === GATEWAY_ROLE)
    .map((row): HolderRowView => ({ grantId: row.id, principalPrn: row.principalPrn, member: membersTruncated ? null : memberPrns.has(row.principalPrn) }))
    .sort((a, b) => (a.principalPrn < b.principalPrn ? -1 : a.principalPrn > b.principalPrn ? 1 : 0));
  const holderPrns = new Set(holders.map((row) => row.principalPrn));
  const pool = new Set([...memberPrns, ...grants.map((row) => row.principalPrn)]);
  const candidates = [...pool]
    .filter((prn) => !holderPrns.has(prn))
    .sort()
    .map((principalPrn) => ({ principalPrn }));
  return { holders, candidates };
}

export async function loadPeopleModelAccess(deps: PeopleModelAccessDeps, params: PeopleModelAccessParams): Promise<PeopleModelAccessView> {
  if (!cedarCapabilityOf(deps.iam)) return { kind: 'disabled' };
  const { orgPrn } = params;
  const [grants, members, mayGrant, mayRevoke] = await Promise.all([
    readBounded((limit, offset) => callIam(async () => (await deps.authz.listRoleGrants({ scopePrn: orgPrn, principalKind: PrincipalKind.USER, limit, offset })).grants)),
    readBounded((limit, offset) =>
      callIam(async () => (await deps.tenancy.listMemberships({ filter: { case: 'nodePrn', value: orgPrn }, principalKind: PrincipalKind.USER, limit, offset })).memberships),
    ),
    deps.mayI('GrantRole', orgPrn),
    deps.mayI('RevokeRole', orgPrn),
  ]);
  if ((!grants.ok && grants.error.presentation === 'forbidden') || (!members.ok && members.error.presentation === 'forbidden')) return { kind: 'denied' };
  if (!grants.ok) return { kind: 'error', error: grants.error };
  if (!members.ok) return { kind: 'error', error: members.error };

  const { holders, candidates } = joinPeople(grants.value.rows, members.value.rows, members.value.truncated);
  return {
    kind: 'ok',
    orgPrn,
    readOnly: !params.orgActive,
    holders,
    candidates,
    flags: { canGrant: params.orgActive && mayGrant, canRevoke: params.orgActive && mayRevoke, grantsTruncated: grants.value.truncated, membersTruncated: members.value.truncated },
  };
}
