// SPDX-License-Identifier: Apache-2.0
//
// The Members section of every node page (spec § 5.2). A node page is the only place where IAM
// lets a non-admin list memberships: ListMemberships(node_prn) checks against the node.
import 'server-only';
import type { MayI } from '../../../lib/authorize';
import { callIam, type IamResult } from '../../../lib/errors';
import type { IamClients } from '../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../lib/paging';

export type MemberRow = { readonly id: string; readonly principalPrn: string };
export type MemberList = { readonly rows: readonly MemberRow[]; readonly offset: number; readonly nextOffset: number | null };
export type MembersData = { readonly list: IamResult<MemberList>; readonly canAttach: boolean; readonly canDetach: boolean };
export type MembersDeps = { readonly tenancy: Pick<IamClients['tenancy'], 'listMemberships'>; readonly mayI: MayI };

export async function loadMembers(deps: MembersDeps, nodePrn: string, offset: number): Promise<MembersData> {
  const [result, canAttach, canDetach] = await Promise.all([
    callIam(() => deps.tenancy.listMemberships({ filter: { case: 'nodePrn', value: nodePrn }, limit: PAGE_SIZE, offset: BigInt(offset) })),
    deps.mayI('AttachMembership', nodePrn),
    deps.mayI('DetachMembership', nodePrn),
  ]);
  const list: IamResult<MemberList> = result.ok
    ? {
        ok: true,
        value: {
          rows: result.value.memberships.map((membership) => ({ id: membership.id, principalPrn: membership.principalPrn })),
          offset,
          nextOffset: nextOffset(offset, result.value.memberships.length),
        },
      }
    : result;
  return { list, canAttach, canDetach };
}
