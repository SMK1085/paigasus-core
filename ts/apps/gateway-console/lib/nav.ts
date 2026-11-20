// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries, and the one that builds the console
// switcher's (SMA-783). Every entry's `state` comes from
// @paigasus/app-shell's navStateOf(): no entry here decides its own visibility from a capability
// list.
//
// Hrefs are FULL paths, as the ingress sees them (SMA-510 spec § 6.2).
import 'server-only';
import { navStateOf, type NavEntry, type ZoneMap } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

/**
 * This image IS the gateway zone (next.config.ts's `basePath: '/gateway'`), so this app's own
 * prefix is compiled in, not read from the zone map: it can never be anything else, and
 * @paigasus/next-config/runtime already fails closed at first request if the deployment's zone map
 * disagrees. Only a genuinely cross-zone base path (`iam`, below) needs a runtime lookup. This
 * mirrors iam-console/lib/nav.ts and departs from spec § 7.1's literal wording (plan D17).
 */
export const GATEWAY_BASE_PATH = '/gateway';

export function buildNavEntries(input: { gateway: ServiceState }): NavEntry[] {
  // Since SMA-783 the Primary nav lists this console's pages only; IAM is reached through the
  // console switcher (buildConsoleEntries, below).
  return [{ zone: 'gateway', href: `${GATEWAY_BASE_PATH}/overview`, label: 'Overview', state: navStateOf(input.gateway) }];
}

/**
 * The console switcher's entries (SMA-783): every console, this one included, in the same order in
 * every zone (the mirror image of iam-console/lib/nav.ts). The switcher drops a console whose zone
 * is not in the map (its rule 1), and navStateOf answers `absent` for a service that is not configured.
 */
export function buildConsoleEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap }): NavEntry[] {
  const entries: NavEntry[] = [];
  const iamBase = input.zones['iam'];
  if (iamBase !== undefined) {
    entries.push({ zone: 'iam', href: `${iamBase}/orgs`, label: 'IAM', state: navStateOf(input.iam) });
  }
  entries.push({ zone: 'gateway', href: `${GATEWAY_BASE_PATH}/overview`, label: 'AI Gateway', state: navStateOf(input.gateway) });
  return entries;
}

/** What a "Manage in IAM" link points at (SMA-636 spec § 4.4). */
export type ManageTarget = { readonly kind: 'organization'; readonly orgId: string } | { readonly kind: 'project'; readonly orgId: string; readonly teamId: string | null; readonly projectId: string };

/**
 * The "Manage in IAM" href (SMA-636 spec § 4.4, D10): the same node in the IAM zone, from the zone
 * map's `iam` entry, in the pattern of buildNavEntries above. Null when the map has no IAM zone
 * (the single-zone e2e tier), and for a project whose team is not known, because the IAM zone's
 * project route carries the team.
 */
export function iamManageHref(zones: ZoneMap, target: ManageTarget): string | null {
  const iamBase = zones['iam'];
  if (iamBase === undefined) return null;
  if (target.kind === 'organization') return `${iamBase}/orgs/${target.orgId}`;
  return target.teamId === null ? null : `${iamBase}/orgs/${target.orgId}/teams/${target.teamId}/projects/${target.projectId}`;
}
