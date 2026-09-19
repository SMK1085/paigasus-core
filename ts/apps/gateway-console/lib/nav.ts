// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries. Every entry's `state` comes from
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

export function buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap }): NavEntry[] {
  const entries: NavEntry[] = [{ zone: 'gateway', href: `${GATEWAY_BASE_PATH}/overview`, label: 'Overview', state: navStateOf(input.gateway) }];
  // A cross-zone entry. PrimaryNav drops it when `iam` is not a zone (its rule 1), and navStateOf
  // answers `absent` when `iam` is not a configured service. The mirror image of iam-console's own
  // gateway entry, so the link appears in both directions as soon as the zone map names both.
  const iamBase = input.zones['iam'];
  if (iamBase !== undefined) {
    entries.push({ zone: 'iam', href: `${iamBase}/orgs`, label: 'IAM', state: navStateOf(input.iam) });
  }
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
