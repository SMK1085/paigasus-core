// SPDX-License-Identifier: Apache-2.0
//
// /gateway/orgs/[org] (spec § 7.3). This route is why forbidden.tsx is reachable in this zone: a
// denied GetOrganization produces the `forbidden` presentation, PageError calls forbidden(), and
// Next renders the 403 boundary inside the console layout.
//
// Recorded limit (spec § 13): the scope changes the URL and the breadcrumbs and nothing else. The
// route exists now so that the later settings screens hang off a shape that already works, and so
// the organization switcher is not a dead control.
import type { ReactElement } from 'react';
import { notFound } from 'next/navigation';
import { Breadcrumbs } from '@paigasus/app-shell';
import { callIam, isUuid, organizationPrn } from '@paigasus/console-core';
import { PageError } from '../../../_components/page-error';
import { gatewayView } from '../../../_components/gateway-state';
import { ZoneOverview } from '../../../_components/zone-overview';
import { currentSession, discovery, iamClients } from '../../../../lib/console';
import { GATEWAY_BASE_PATH } from '../../../../lib/nav';

export default async function ScopePage({ params }: { params: Promise<{ org: string }> }): Promise<ReactElement> {
  const { org } = await params;
  if (!isUuid(org)) notFound();
  const orgId = org.toLowerCase();
  const [session, clients] = await Promise.all([currentSession(), iamClients()]);
  // The organization read comes FIRST: when IAM denies it, the page is the 403 view and nothing
  // else runs. The PRN comes from the URL, so IAM's invalid-input answer means the URL names no
  // such node — notFound(), not an error view.
  const got = await callIam(() => clients.tenancy.getOrganization({ prn: organizationPrn(orgId) }));
  if (!got.ok) return got.error.presentation === 'invalid-input' ? notFound() : <PageError error={got.error} />;
  const organization = got.value.organization;
  if (organization === undefined) notFound();
  const state = await discovery().getServiceState('gateway', session.accessToken);
  return (
    <div className="flex flex-col gap-8 p-6">
      <Breadcrumbs items={[{ label: 'Overview', href: `${GATEWAY_BASE_PATH}/overview` }, { label: organization.name }]} />
      <ZoneOverview view={gatewayView(state)} scope={{ orgId, name: organization.name }} />
    </div>
  );
}
