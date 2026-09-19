// SPDX-License-Identifier: Apache-2.0
//
// /gateway/overview (spec § 6, plan D14; SMA-636 D16). currentSession(), discovery() and myScopes()
// are React cache() wrappers built by the one createConsoleRuntime call, and Discovery memoizes
// getServiceState per handle — so this page and the layout above it share ONE session resolution,
// ONE gateway probe and ONE myScopes() walk per request. "Your projects" therefore costs no call.
// That sharing is exactly what a second createConsoleRuntime() call would break, invisibly.
//
// This route is /gateway/overview, NOT /gateway/: (public)/page.tsx already owns the latter, and
// two page.tsx at one path fail the Next build (plan D14).
import type { ReactElement } from 'react';
import { gatewayView } from '../../_components/gateway-state';
import { YourProjects } from '../../_components/your-projects';
import { ZoneOverview } from '../../_components/zone-overview';
import { currentSession, discovery, myScopes } from '../../../lib/console';
import { GATEWAY_BASE_PATH } from '../../../lib/nav';

export default async function OverviewPage(): Promise<ReactElement> {
  const session = await currentSession();
  const [state, scopes] = await Promise.all([discovery().getServiceState('gateway', session.accessToken), myScopes()]);
  return (
    <div className="flex flex-col">
      <ZoneOverview view={gatewayView(state)} />
      <div className="px-8 pb-8">
        <YourProjects scopes={scopes} basePath={GATEWAY_BASE_PATH} />
      </div>
    </div>
  );
}
