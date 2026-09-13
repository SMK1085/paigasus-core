// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries (spec § 5.4). Every entry's `state` comes from
// @paigasus/app-shell's navStateOf(), which closes SMA-510 spec § 10.4's second gap: no entry here
// decides its own visibility from a capability list.
//
// Hrefs are FULL paths, as the ingress sees them (SMA-510 spec § 6.2).
import 'server-only';
import { navStateOf, type NavEntry, type ZoneMap } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

/**
 * This image IS the IAM zone (next.config.ts's `basePath: '/iam'`), so this app's own prefix is
 * compiled in, not read from the zone map: it can never be anything else, and
 * @paigasus/next-config/runtime already fails closed at first request if the deployment's zone
 * map disagrees. Only a genuinely cross-zone base path (`gateway`, below) needs a runtime lookup.
 */
export const IAM_BASE_PATH = '/iam';

export function buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap; auditAllowed: boolean }): NavEntry[] {
  const entries: NavEntry[] = [{ zone: 'iam', href: `${IAM_BASE_PATH}/orgs`, label: 'Organizations', state: navStateOf(input.iam) }];
  // ListAuditEntries is Root-only (rs/crates/services/paigasus-iam/src/application/audit.rs:36-38),
  // so the layout asks mayI('ListAuditLog', ROOT_PRN) and omits the entry on a clear "no". The
  // capability decides the rest: absent without `iam.audit`, disabled when IAM is degraded.
  if (input.auditAllowed) {
    entries.push({ zone: 'iam', href: `${IAM_BASE_PATH}/audit`, label: 'Audit', state: navStateOf(input.iam, 'iam.audit') });
  }
  // A cross-zone entry. PrimaryNav drops it when `gateway` is not a zone (its rule 1), and
  // navStateOf answers `absent` when `gateway` is not a configured service.
  const gatewayBase = input.zones['gateway'];
  if (gatewayBase !== undefined) {
    entries.push({ zone: 'gateway', href: `${gatewayBase}/`, label: 'Gateway', state: navStateOf(input.gateway) });
  }
  return entries;
}
