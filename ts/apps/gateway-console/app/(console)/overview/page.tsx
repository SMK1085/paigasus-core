// SPDX-License-Identifier: Apache-2.0
//
// /gateway/overview (spec § 6, plan D14). currentSession() and discovery() are both React cache()
// wrappers built by the one createConsoleRuntime call, and Discovery memoizes getServiceState per
// handle — so this page and the layout above it share ONE session resolution and ONE gateway probe
// per request. That sharing is exactly what a second createConsoleRuntime() call would break,
// invisibly (Global Constraints).
//
// This route is /gateway/overview, NOT /gateway/: (public)/page.tsx already owns the latter, and
// two page.tsx at one path fail the Next build (plan D14).
import type { ReactElement } from 'react';
import { gatewayView } from '../../_components/gateway-state';
import { ZoneOverview } from '../../_components/zone-overview';
import { currentSession, discovery } from '../../../lib/console';

export default async function OverviewPage(): Promise<ReactElement> {
  const session = await currentSession();
  const state = await discovery().getServiceState('gateway', session.accessToken);
  return <ZoneOverview view={gatewayView(state)} />;
}
