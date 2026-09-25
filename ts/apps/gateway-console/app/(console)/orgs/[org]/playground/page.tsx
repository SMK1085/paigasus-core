// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org]/playground (SMA-635 spec § 6.1). The same head as the org page: a value that
// is not a UUID is not-found, and a GetOrganization denial or absence gives the org page's
// not-found or 403 view. The gateway's discovery state decides the composer (D7).
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { isUuid } from '@paigasus/console-core';
import { gatewayView } from '../../../../_components/gateway-state';
import { PageError } from '../../../../_components/page-error';
import { Playground } from '../../../../_components/playground';
import { composerNotice } from '../../../../_components/playground-notice';
import { GATEWAY_BASE_PATH } from '../../../../../lib/nav';
import { loadSettingsPrelude } from '../../../settings-prelude';
import { loadOrganizationHead } from '../load';

type Props = { params: Promise<{ org: string }> };

export default async function PlaygroundPage({ params }: Props): Promise<ReactElement> {
  const { org } = await params;
  if (!isUuid(org)) notFound();
  const { clients, gateway } = await loadSettingsPrelude();
  const head = await loadOrganizationHead(clients.tenancy, org);
  if (head.kind === 'not-found') notFound();
  if (head.kind === 'error') return <PageError error={head.error} />;
  return (
    <div className="flex flex-col gap-6 p-6" data-testid="playground-page">
      <Breadcrumbs
        items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: head.organization.name, href: `${GATEWAY_BASE_PATH}/orgs/${head.orgId}` }, { label: 'Playground' }]}
      />
      <h1 className="text-xl font-semibold">Playground</h1>
      <Playground orgId={head.orgId} notice={composerNotice(gatewayView(gateway))} />
    </div>
  );
}
