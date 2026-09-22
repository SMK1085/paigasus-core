// SPDX-License-Identifier: Apache-2.0
//
// IAM tenancy PRNs for the console, over the kernel's PRN grammar (SMA-634 spec § 6.2, ADR-0022).
//
// NOT an ADR-0005 exception any more. The grammar lives once, in paigasus-kernel
// (rs/crates/libs/paigasus-kernel/src/resource_name.rs), and this module calls it through
// `@paigasus/kernel`, whose `.` export is the platform-neutral wasm entry. What stays here is IAM's
// own tenancy rule (rs/crates/libs/paigasus-iam-core/src/tenancy.rs `check`): service `iam`; an
// organization has NO org field (its org id is its own id); a team and a project have one.
//
// tests/unit/prn-tenancy.test.ts replays the kernel parity corpus through this file, and
// tests/unit/prn-tenancy-delegation.test.ts proves that the kernel, not this file, reads the PRN.
import 'server-only';
import { prnBuild, prnErrorKind, prnOrg, prnRegion, prnResourceId, prnResourceType, prnService } from '@paigasus/kernel';

export type TenancyKind = 'organization' | 'team' | 'project';
export type TenancyRef = { kind: TenancyKind; orgId: string; id: string };

/** The canonical PRN of IAM's synthetic Cedar Root: `root_prn()` in paigasus-iam-core authz/model.rs. */
export const ROOT_PRN = 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000';

/**
 * A RESOURCE LIMIT, not grammar. The kernel enforces its own 512-byte rule, but it does so AFTER the
 * string is copied into wasm linear memory, and that memory never shrinks. So an unbounded caller
 * input is bounded here, before the first kernel call. Do not remove this as a duplicate of the
 * kernel's MAX_LEN.
 */
const MAX_LEN = 512;

/**
 * The UUID shape of a URL segment. This is NOT PRN grammar: the kernel exposes no UUID predicate,
 * and the pages use this to reject a bad URL segment before they build a PRN from it.
 */
const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

const TENANCY_KINDS: ReadonlySet<string> = new Set<TenancyKind>(['organization', 'team', 'project']);

export function isUuid(value: string): boolean {
  return UUID.test(value);
}

function requireUuid(label: string, value: string): string {
  if (!isUuid(value)) throw new TypeError(`${label} must be a UUID`);
  return value.toLowerCase();
}

/**
 * The tenancy node a PRN names, or null for any other resource and for an invalid PRN. The kernel
 * decides validity and returns the canonical, lower-case fields.
 *
 * A NON-EMPTY REGION IS REJECTED, well formed or not. `TenancyRef` has no region field and the three
 * builders always emit an empty one, so a regionful PRN read here would be REWRITTEN without its
 * region on the way back out: a silently different resource. IAM's own tenancy PRNs carry no region,
 * so this console loses no valid input. The day IAM regionalises tenancy, this returns null instead
 * of corrupting the value, and `TenancyRef` grows a region field.
 */
export function parseTenancyPrn(prn: string): TenancyRef | null {
  if (prn.length === 0 || prn.length > MAX_LEN) return null;
  // The kernel is the grammar: a non-empty kind is any malformed PRN.
  if (prnErrorKind(prn) !== '') return null;
  if (prnService(prn) !== 'iam' || prnRegion(prn) !== '') return null;
  const type = prnResourceType(prn);
  if (!TENANCY_KINDS.has(type)) return null;
  const kind = type as TenancyKind;
  const id = prnResourceId(prn);
  // prnOrg returns '' for an ABSENT org field; a malformed one is already an error kind above.
  const org = prnOrg(prn);
  if (kind === 'organization') return org === '' ? { kind, orgId: id, id } : null;
  return org === '' ? null : { kind, orgId: org, id };
}

export function organizationPrn(orgId: string): string {
  return prnBuild('iam', '', '', 'organization', requireUuid('orgId', orgId));
}

export function teamPrn(orgId: string, teamId: string): string {
  return prnBuild('iam', '', requireUuid('orgId', orgId), 'team', requireUuid('teamId', teamId));
}

export function projectPrn(orgId: string, projectId: string): string {
  return prnBuild('iam', '', requireUuid('orgId', orgId), 'project', requireUuid('projectId', projectId));
}
