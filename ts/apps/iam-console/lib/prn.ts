// SPDX-License-Identifier: Apache-2.0
//
// IAM tenancy PRNs for the console (SMA-511 spec § 4.7, decision D6, fallback C).
//
// A RECORDED ADR-0005 EXCEPTION (spec § 12). PRN logic belongs to paigasus-kernel. The kernel's napi
// binding cannot load in a Next build (spec § 13 row 5), and the wasm spike did not pass (spec § 13
// row 10). So this module holds a small reader for the THREE tenancy shapes only. It is held to the
// kernel by tests/unit/prn.test.ts, which runs every vector of the kernel parity corpus
// (rs/crates/libs/paigasus-kernel-parity/vectors/) through it: a divergence from the kernel fails CI.
//
// The grammar is the kernel's (rs/crates/libs/paigasus-kernel/src/resource_name.rs):
//   prn:pgs:<service>:<region>:<org>:<resource-type>/<resource-id>
// The tenancy rule is IAM's (rs/crates/libs/paigasus-iam-core/src/tenancy.rs `check`): service
// `iam`; an organization has NO org field (its org id is its own id); a team and a project have one.
import 'server-only';

export type TenancyKind = 'organization' | 'team' | 'project';
export type TenancyRef = { kind: TenancyKind; orgId: string; id: string };

/** The canonical PRN of IAM's synthetic Cedar Root: `root_prn()` in paigasus-iam-core authz/model.rs. */
export const ROOT_PRN = 'prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000';

/**
 * The kernel's MAX_LEN is 512 BYTES. This compares UTF-16 units; the two differ only for non-ASCII
 * input, which no valid field can hold, so such a string is rejected either way.
 */
const MAX_LEN = 512;

/** The kernel's UUID field form: 36 characters, hyphenated, either case (resource_name.rs `parse_uuid_field`). */
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
 * The tenancy node a PRN names, or null for any other resource or an invalid PRN. Ids are
 * lower-case.
 *
 * A NON-EMPTY REGION IS REJECTED, well formed or not — which is why this file needs no copy of the
 * kernel's `is_valid_region` grammar. `TenancyRef` has no region field and the three builders always
 * emit an empty one, so a regionful PRN read here would be REWRITTEN without its region on the way
 * back out: a silently different resource. IAM's own tenancy PRNs carry no region
 * (`paigasus-iam-core` tenancy.rs), so this console loses no valid input. The day IAM regionalises
 * tenancy, this returns null instead of corrupting the value, and `TenancyRef` grows a region field.
 */
export function parseTenancyPrn(prn: string): TenancyRef | null {
  if (prn.length === 0 || prn.length > MAX_LEN) return null;
  const parts = prn.split(':');
  if (parts.length !== 6) return null;
  const [scheme, partition, service, region, org, path] = parts as [string, string, string, string, string, string];
  // `iam` is a valid kernel service label, so checking equality also checks the label grammar.
  if (scheme !== 'prn' || partition !== 'pgs' || service !== 'iam') return null;
  if (region !== '') return null;
  if (org !== '' && !isUuid(org)) return null;
  const slash = path.indexOf('/');
  if (slash === -1 || path.includes('/', slash + 1)) return null;
  const type = path.slice(0, slash);
  const id = path.slice(slash + 1);
  // The three tenancy names are valid kernel resource-type labels, so membership also checks the grammar.
  if (!TENANCY_KINDS.has(type) || !isUuid(id)) return null;
  const kind = type as TenancyKind;
  const resourceId = id.toLowerCase();
  if (kind === 'organization') return org === '' ? { kind, orgId: resourceId, id: resourceId } : null;
  return org === '' ? null : { kind, orgId: org.toLowerCase(), id: resourceId };
}

export function organizationPrn(orgId: string): string {
  return `prn:pgs:iam:::organization/${requireUuid('orgId', orgId)}`;
}

export function teamPrn(orgId: string, teamId: string): string {
  return `prn:pgs:iam::${requireUuid('orgId', orgId)}:team/${requireUuid('teamId', teamId)}`;
}

export function projectPrn(orgId: string, projectId: string): string {
  return `prn:pgs:iam::${requireUuid('orgId', orgId)}:project/${requireUuid('projectId', projectId)}`;
}
