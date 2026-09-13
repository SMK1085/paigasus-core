// SPDX-License-Identifier: Apache-2.0
//
// Every page that needs a session (spec § 5.4). It resolves the session FIRST — requireSession()
// redirects to login when there is none — then builds the shell from five request-scoped reads.
//
// This layout does NOT guard Server Actions: an action is its own request, and each one gets its
// token through iamClients(), which calls requireSession() itself (spec § 3.3, § 5.3).
//
// SessionProvider receives toSessionView() output only — never a token (ADR-0017). This is the
// first real Flight handoff of getPublicConfig().zones to ZoneProvider (SMA-510 spec § 10.4).
//
// The organization switcher lists the organizations from myScopes() (spec § 5.4). myScopes() is a
// React cache(), so in a Next request the /orgs page and this layout share ONE call. OrgSwitcherShell
// is a client component: it gets plain data only, and reads the [org] segment with useParams().
import type { ReactElement, ReactNode } from 'react';
import { connection } from 'next/server';
import { toSessionView } from '@paigasus/auth/server';
import { buildNavEntries, IAM_BASE_PATH } from '../../lib/nav';
import { getPublicConfig } from '../../lib/config';
import { discovery } from '../../lib/discovery';
import { currentSession } from '../../lib/iam';
import { mayI } from '../../lib/authorize';
import { ROOT_PRN } from '../../lib/prn-tenancy';
import { myScopes, switcherOrgs } from '../../lib/scopes';
import { OrgSwitcherShell } from '../_components/org-switcher';
import { Providers } from '../providers';

export default async function ConsoleLayout({ children }: { children: ReactNode }): Promise<ReactElement> {
  // No prerender, as in app/(public)/layout.tsx: the runtime config does not exist during `next build`.
  await connection();
  const session = await currentSession();
  const { zone, zones } = getPublicConfig();
  const probe = discovery();
  const [iam, gateway, may, scopes] = await Promise.all([probe.getServiceState('iam', session.accessToken), probe.getServiceState('gateway', session.accessToken), mayI(), myScopes()]);
  const nav = buildNavEntries({ iam, gateway, zones, auditAllowed: await may('ListAuditLog', ROOT_PRN) });
  return (
    <Providers zone={zone} zones={zones} session={toSessionView(session)}>
      <OrgSwitcherShell brand={{ label: 'Paigasus IAM', href: `${IAM_BASE_PATH}/orgs` }} nav={nav} orgs={switcherOrgs(scopes)}>
        {children}
      </OrgSwitcherShell>
    </Providers>
  );
}
