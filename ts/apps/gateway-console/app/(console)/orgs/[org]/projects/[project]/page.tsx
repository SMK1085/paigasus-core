// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org]/projects/[project] — the project settings (SMA-636 spec § 4.1, § 4.2, D5: a
// flat route with no team segment). The header names the project, its slug, its team and its
// status; the breadcrumbs are Overview › <org> › <project>. A failed GetTeam or GetOrganization
// shows the UUID instead of the name. A URL that pairs one organization with another
// organization's project answers 403 or 404 and never shows the project (§ 4.1).
//
// Rename and archive of the project stay in the IAM zone (D10).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { isUuid } from '@paigasus/console-core';
import { gatewayView } from '../../../../../_components/gateway-state';
import { GatewayStateLine } from '../../../../../_components/gateway-state-line';
import { PageError } from '../../../../../_components/page-error';
import { SettingsHeader } from '../../../../../_components/settings-header';
import { getPublicConfig } from '../../../../../../lib/config';
import { GATEWAY_BASE_PATH, iamManageHref } from '../../../../../../lib/nav';
import { parseOffset } from '../../../../../../lib/paging';
import { statusColumnLabel } from '../../../../node-status';
import { loadSettingsPrelude } from '../../../../settings-prelude';
import { serviceAccountsBlock } from '../../../../service-accounts/block';
import { parseAccountParam } from '../../../../service-accounts/service-account-id';
import { loadProjectSettings } from './load';

type Props = {
  params: Promise<{ org: string; project: string }>;
  searchParams: Promise<Record<string, string | string[] | undefined>>;
};

export default async function ProjectSettingsPage({ params, searchParams }: Props): Promise<ReactElement> {
  const [{ org, project }, query] = await Promise.all([params, searchParams]);
  if (!isUuid(org) || !isUuid(project)) notFound();
  const { clients, may, iam, gateway } = await loadSettingsPrelude();
  const data = await loadProjectSettings(
    { tenancy: clients.tenancy, serviceAccounts: clients.serviceAccounts, authz: clients.authz, mayI: may, iam, now: Date.now },
    { org, project, saOffset: parseOffset(query['saOffset']), keyOffset: parseOffset(query['keyOffset']), sa: parseAccountParam(query['sa']) },
  );
  if (data.kind === 'not-found') notFound();
  if (data.kind === 'error') return <PageError error={data.error} />;

  const orgPath = `${GATEWAY_BASE_PATH}/orgs/${data.orgId}`;
  const path = `${orgPath}/projects/${data.projectId}`;
  const manage = iamManageHref(getPublicConfig().zones, { kind: 'project', orgId: data.orgId, teamId: data.team.id, projectId: data.projectId });
  return (
    <div className="flex flex-col gap-8 p-6" data-testid="project-settings">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: data.organizationName ?? data.orgId, href: orgPath }, { label: data.project.name }]} />
      <SettingsHeader
        name={data.project.name}
        status={statusColumnLabel(data.project.lifecycle)}
        slug={data.project.slug}
        extra={
          <p data-testid="project-team" className="text-sm">
            {`Team: ${data.team.name ?? data.team.id ?? 'unknown'}`}
          </p>
        }
        manage={manage}
      />
      <GatewayStateLine view={gatewayView(gateway)} />
      {await serviceAccountsBlock({ view: data.section, ownerKind: 'project', ownerPrn: data.projectPrn, path })}
    </div>
  );
}
