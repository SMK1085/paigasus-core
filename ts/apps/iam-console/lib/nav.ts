// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries (spec § 5.4). Every entry's `state` comes from
// @paigasus/app-shell's navStateOf(), which closes SMA-510 spec § 10.4's second gap: no entry here
// decides its own visibility from a capability list.
//
// Hrefs are FULL paths, as the ingress sees them (SMA-510 spec § 6.2), built from the zone map, so a
// zone mounted at another prefix gets the right link without a code change.
import 'server-only';
import { navStateOf, type NavEntry, type ZoneMap } from '@paigasus/app-shell';
import type { ServiceState } from '@paigasus/discovery/types';

export function buildNavEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap; auditAllowed: boolean }): NavEntry[] {
  const iamBase = input.zones['iam'];
  if (iamBase === undefined || !Object.hasOwn(input.zones, 'iam')) {
    // getRuntimeConfig() already refuses a zone map without this app's own zone, so this is a
    // programming error, not a deployment one.
    throw new Error('buildNavEntries: the zone map has no "iam" entry');
  }
  const entries: NavEntry[] = [{ zone: 'iam', href: `${iamBase}/orgs`, label: 'Organizations', state: navStateOf(input.iam) }];
  // ListAuditEntries is Root-only (rs/crates/services/paigasus-iam/src/application/audit.rs:36-38),
  // so the layout asks mayI('ListAuditLog', ROOT_PRN) and omits the entry on a clear "no". The
  // capability decides the rest: absent without `iam.audit`, disabled when IAM is degraded.
  if (input.auditAllowed) {
    entries.push({ zone: 'iam', href: `${iamBase}/audit`, label: 'Audit', state: navStateOf(input.iam, 'iam.audit') });
  }
  // A cross-zone entry. PrimaryNav drops it when `gateway` is not a zone (its rule 1), and
  // navStateOf answers `absent` when `gateway` is not a configured service.
  const gatewayBase = input.zones['gateway'];
  if (gatewayBase !== undefined && Object.hasOwn(input.zones, 'gateway')) {
    entries.push({ zone: 'gateway', href: `${gatewayBase}/`, label: 'Gateway', state: navStateOf(input.gateway) });
  }
  return entries;
}
