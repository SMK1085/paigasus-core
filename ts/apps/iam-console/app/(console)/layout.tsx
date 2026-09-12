// SPDX-License-Identifier: Apache-2.0
//
// Every page that needs a session (spec § 5.4). It resolves the session FIRST — requireSession()
// redirects to login when there is none — then builds the shell from four request-scoped reads.
//
// This layout does NOT guard Server Actions: an action is its own request, and each one gets its
// token through iamClients(), which calls requireSession() itself (spec § 3.3, § 5.3).
//
// SessionProvider receives toSessionView() output only — never a token (ADR-0017). This is the
// first real Flight handoff of getPublicConfig().zones to ZoneProvider (SMA-510 spec § 10.4).
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { AppShell } from '@paigasus/app-shell';
import { toSessionView } from '@paigasus/auth/server';
import { buildNavEntries } from '../../lib/nav';
import { getPublicConfig } from '../../lib/config';
import { discovery } from '../../lib/discovery';
import { currentSession } from '../../lib/iam';
import { mayI } from '../../lib/authorize';
import { ROOT_PRN } from '../../lib/prn';
import { Providers } from '../providers';

export default async function ConsoleLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  // No prerender, as in app/(public)/layout.tsx: the runtime config does not exist during `next build`.
  await connection();
  const session = await currentSession();
  const { zone, zones } = getPublicConfig();
  const probe = discovery();
  const [iam, gateway, may] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken), mayI()]);
  const nav = buildNavEntries({ iam, gateway, zones, auditAllowed: await may('ListAuditLog', ROOT_PRN) });
  const iamBase = zones['iam'] ?? '/iam';
  // No organization switcher yet: it needs myScopes() (the SMA-511 plan's Task 16), which then
  // replaces AppShell with OrgSwitcherShell (app/_components/org-switcher.tsx) and the same props.
  return (
    <Providers zone={zone} zones={zones} session={toSessionView(session)}>
      <AppShell brand={{ label: 'Paigasus IAM', href: `${iamBase}/orgs` }} nav={nav}>
        {children}
      </AppShell>
    </Providers>
  );
}
