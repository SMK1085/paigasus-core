// SPDX-License-Identifier: Apache-2.0
//
// The authenticated shell with its organization switcher (spec § 5.4). CLIENT component, for one
// reason: a layout does not receive the params of its child segments, so only a client component
// can read the `[org]` segment, through useParams(). The selection lives in the URL (SMA-510
// spec § 4, D1); nothing here stores it.
//
// The two helpers below are exported for the unit test only. They live in a 'use client' module,
// so a SERVER component that imported them would get client references, not functions.
'use client';

import type { ReactElement, ReactNode } from 'react';
import { useParams } from 'next/navigation';
import { AppShell, useZone, type Brand, type NavEntry, type SwitcherItem } from '@paigasus/app-shell';

/** One organization the switcher can list. Built on the server from myScopes(). */
export type OrgSwitcherOrg = { readonly orgId: string; readonly label: string };

/** Switcher items with FULL-path hrefs, as @paigasus/app-shell requires (SMA-510 spec § 6.2). */
export function orgSwitcherItems(orgs: readonly OrgSwitcherOrg[], basePath: string): SwitcherItem[] {
  return orgs.map((org) => ({ id: org.orgId, label: org.label, href: `${basePath}/orgs/${org.orgId}` }));
}

/**
 * The `[org]` segment of the current URL in lower case, or null on a page outside one organization.
 * The page accepts an upper-case UUID, and the switcher's item ids are lower case (myScopes()).
 */
export function currentOrgId(params: Readonly<Record<string, string | string[] | undefined>> | null): string | null {
  const org = params?.['org'];
  return typeof org === 'string' ? org.toLowerCase() : null;
}

/**
 * AppShell plus one "Organization" switcher. The same props as AppShell, with `orgs` in place of
 * `switchers`: AppShell renders a switcher only from its `switchers` prop, so the component that
 * knows the current `[org]` (a client, through useParams()) must be the one that renders AppShell.
 */
export function OrgSwitcherShell({ brand, nav, orgs, children }: { brand: Brand; nav: readonly NavEntry[]; orgs: readonly OrgSwitcherOrg[]; children: ReactNode }): ReactElement {
  const params = useParams();
  const { basePath } = useZone();
  const items = orgSwitcherItems(orgs, basePath);
  return (
    <AppShell brand={brand} nav={nav} switchers={items.length === 0 ? [] : [{ label: 'Organization', items, currentId: currentOrgId(params) }]}>
      {children}
    </AppShell>
  );
}
