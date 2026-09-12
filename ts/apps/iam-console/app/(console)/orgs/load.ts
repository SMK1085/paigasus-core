// SPDX-License-Identifier: Apache-2.0
//
// The loader of /iam/orgs (spec § 5.2). It takes ports, so the tier-2 tests call it with a client
// to the fake IAM and a scripted mayI. The "Your organizations" data is lib/scopes.ts' myScopes().
import 'server-only';
import type { MayI } from '../../../lib/authorize';
import { callIam, type IamResult } from '../../../lib/errors';
import type { IamClients } from '../../../lib/iam';
import { PAGE_SIZE, nextOffset } from '../../../lib/paging';
import { ROOT_PRN, parseTenancyPrn } from '../../../lib/prn';

export type OrganizationRow = { readonly prn: string; readonly orgId: string | null; readonly slug: string; readonly name: string };
export type OrganizationList = { readonly rows: readonly OrganizationRow[]; readonly offset: number; readonly nextOffset: number | null };
export type OrganizationsPageData = {
  readonly canCreateOrganization: boolean;
  /** `null`: the section is hidden, because mayI('ListOrganizations', root) said no. */
  readonly all: IamResult<OrganizationList> | null;
};
export type OrganizationsPageDeps = {
  readonly tenancy: Pick<IamClients['tenancy'], 'listOrganizations'>;
  readonly mayI: MayI;
};

export async function loadOrganizationsPage(deps: OrganizationsPageDeps, params: { readonly offset: number }): Promise<OrganizationsPageData> {
  // Both RPCs check against the root PRN (adapters/grpc/tenancy.rs:135 and :193).
  const [canList, canCreateOrganization] = await Promise.all([deps.mayI('ListOrganizations', ROOT_PRN), deps.mayI('CreateOrganization', ROOT_PRN)]);
  if (!canList) return { canCreateOrganization, all: null };

  const result = await callIam(() => deps.tenancy.listOrganizations({ limit: PAGE_SIZE, offset: BigInt(params.offset) }));
  if (!result.ok) return { canCreateOrganization, all: result };

  const rows = result.value.organizations.map((organization): OrganizationRow => {
    const ref = parseTenancyPrn(organization.prn);
    return { prn: organization.prn, orgId: ref?.kind === 'organization' ? ref.id.toLowerCase() : null, slug: organization.slug, name: organization.name };
  });
  return { canCreateOrganization, all: { ok: true, value: { rows, offset: params.offset, nextOffset: nextOffset(params.offset, rows.length) } } };
}
