// SPDX-License-Identifier: Apache-2.0
//
// The ONE function that builds PrimaryNav's entries (spec § 5.4), and the one that builds the
// console switcher's (SMA-783). Every entry's `state` comes from
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

export function buildNavEntries(input: { iam: ServiceState; auditAllowed: boolean; deadLettersAllowed: boolean }): NavEntry[] {
  // Since SMA-783 the Primary nav lists this console's pages only, in groups; the Gateway is
  // reached through the console switcher (buildConsoleEntries, below).
  const entries: NavEntry[] = [{ zone: 'iam', href: `${IAM_BASE_PATH}/orgs`, label: 'Organizations', state: navStateOf(input.iam), group: 'Directory' }];
  // ListAuditEntries is Root-only (rs/crates/services/paigasus-iam/src/application/audit.rs:36-38),
  // so the layout asks mayI('ListAuditLog', ROOT_PRN) and omits the entry on a clear "no". The
  // capability decides the rest: absent without `iam.audit`, disabled when IAM is degraded.
  if (input.auditAllowed) {
    entries.push({ zone: 'iam', href: `${IAM_BASE_PATH}/audit`, label: 'Audit', state: navStateOf(input.iam, 'iam.audit'), group: 'Administration' });
  }
  // SMA-629 § 6.1. Every OutboxService RPC is Root-only, so the layout asks
  // mayI('ListOutboxDeadLetters', ROOT_PRN). mayI() fails open, so during an IAM authz outage a
  // non-Root user can see this entry and IAM refuses the page with a 403, as for Audit. The
  // capability decides the rest: absent without `iam.deadletters` (an older IAM, AC 2), disabled
  // with a reason whenever IAM is degraded, whatever the key (§ 8).
  if (input.deadLettersAllowed) {
    entries.push({ zone: 'iam', href: `${IAM_BASE_PATH}/dead-letters`, label: 'Dead letters', state: navStateOf(input.iam, 'iam.deadletters'), group: 'Administration' });
  }
  return entries;
}

/**
 * The console switcher's entries (SMA-783): every console, this one included, in the same order in
 * every zone (gateway-console/lib/nav.ts mirrors it). The switcher drops a console whose zone is
 * not in the map (its rule 1), and navStateOf answers `absent` for a service that is not configured.
 */
export function buildConsoleEntries(input: { iam: ServiceState; gateway: ServiceState; zones: ZoneMap }): NavEntry[] {
  const entries: NavEntry[] = [{ zone: 'iam', href: `${IAM_BASE_PATH}/orgs`, label: 'IAM', state: navStateOf(input.iam) }];
  const gatewayBase = input.zones['gateway'];
  if (gatewayBase !== undefined) {
    entries.push({ zone: 'gateway', href: `${gatewayBase}/`, label: 'AI Gateway', state: navStateOf(input.gateway) });
  }
  return entries;
}
