// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org] — the organization settings (SMA-636 spec § 4.1, § 4.2). It replaces the
// SMA-512 scope route, which changed only the URL and the breadcrumbs.
//
// GetOrganization first (in the loader): a denial is the 403 view through PageError, and nothing
// else runs; this route is still why forbidden.tsx is reachable in this zone. Then the
// service-accounts section and the Projects list. A section failure never fails the page.
//
// Rename and archive of the organization stay in the IAM zone (D10): the page links there when the
// zone map has an IAM zone (§ 4.4).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { isUuid } from '@paigasus/console-core';
import { gatewayView } from '../../../_components/gateway-state';
import { GatewayStateLine } from '../../../_components/gateway-state-line';
import { PageError } from '../../../_components/page-error';
import { SettingsHeader } from '../../../_components/settings-header';
import { getPublicConfig } from '../../../../lib/config';
import { GATEWAY_BASE_PATH, iamManageHref } from '../../../../lib/nav';
import { parseOffset } from '../../../../lib/paging';
import { statusColumnLabel } from '../../node-status';
import { loadSettingsPrelude } from '../../settings-prelude';
import { serviceAccountsBlock } from '../../service-accounts/block';
import { parseAccountParam } from '../../service-accounts/service-account-id';
import { loadOrganizationSettings } from './load';
import { projectsBlock } from './projects-block';

type Props = {
  params: Promise<{ org: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function OrganizationSettingsPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org)) notFound();
  const { clients, may, iam, gateway } = await loadSettingsPrelude();
  const data = await loadOrganizationSettings(
    { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: may, iam, now: Date.now },
    { org, saOffset: parseOffset(query['saOffset']), keyOffset: parseOffset(query['keyOffset']), sa: parseAccountParam(query['sa']) },
  );
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  const path = `${GATEWAY_BASE_PATH}/orgs/${data.orgId}`;
  const manage = iamManageHref(getPublicConfig().zones, { kind: 'organization', orgId: data.orgId });
  return (
    <div className="flex flex-col gap-8 p-6" data-testid="org-settings">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: data.organization.name }]} />
      <SettingsHeader name={data.organization.name} status={statusColumnLabel(data.organization.lifecycle)} slug={data.organization.slug} manage={manage} />
      <GatewayStateLine view={gatewayView(gateway)} />
      {await serviceAccountsBlock({ view: data.section, ownerKind: 'organization', path })}
      {await projectsBlock({ orgId: data.orgId, projects: data.projects })}
    </div>
  );
}
