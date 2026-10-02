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
// tests/unit/prn-tenancy-delegation.test.ts proves that the kernel, not this file, reads the PRN,
// and that it reads it with ONE kernel call (SMA-673).
import 'server-only';
import { prnBuild, prnParse } from '@paigasus/kernel';
import { logger } from './logger';

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

/** The kernel's reading of a PRN: the fields IAM's tenancy rule needs, all canonical. */
type KernelFields = { resourceType: string; resourceId: string; org: string };

/**
 * The kernel's view of `prn`, or null when the kernel rejects it, when it names another service,
 * when it carries a region, OR WHEN THE KERNEL CALL FAILS.
 *
 * ONE kernel call reads every field (SMA-673), so there is no cross-call invariant to trust. The try
 * is still here because `parseTenancyPrn` returns `TenancyRef | null` and must be TOTAL: a Next
 * server component calls it on a URL segment, and a throw there is a 500 where a 404 belongs, on
 * input an attacker controls. The call can still throw: a wasm runtime failure, or the TypeError
 * that `prnParse` raises on a glue defect. The catch logs that failure, because the log line is the
 * only runtime signal of such a defect. It logs the error's NAME only, never the PRN or the message:
 * redaction is the caller's contract (logger.ts). The catch covers the kernel call ONLY. IAM's
 * tenancy rule stays outside it, in the callers below, so a programming error of ours is never
 * swallowed as "not a tenancy PRN".
 */
function readKernelFields(prn: string): KernelFields | null {
  try {
    // The kernel is the grammar: `ok: false` is any malformed PRN.
    const parsed = prnParse(prn);
    if (!parsed.ok) return null;
    if (parsed.service !== 'iam' || parsed.region !== '') return null;
    // `org` is '' for an ABSENT org field; a malformed one is already `ok: false` above.
    return { resourceType: parsed.resourceType, resourceId: parsed.resourceId, org: parsed.org };
  } catch (error) {
    logger.appEvent('prn.kernel_call_failed', { error: error instanceof Error ? error.name : 'unknown' });
    return null;
  }
}

/**
 * The tenancy node a PRN names, or null for any other resource and for an invalid PRN. The kernel
 * decides validity and returns the canonical, lower-case fields. This function never throws.
 *
 * A NON-EMPTY REGION IS REJECTED, well formed or not. `TenancyRef` has no region field and the three
 * builders always emit an empty one, so a regionful PRN read here would be REWRITTEN without its
 * region on the way back out: a silently different resource. IAM's own tenancy PRNs carry no region,
 * so this console loses no valid input. The day IAM regionalises tenancy, this returns null instead
 * of corrupting the value, and `TenancyRef` grows a region field.
 */
export function parseTenancyPrn(prn: string): TenancyRef | null {
  if (prn.length === 0 || prn.length > MAX_LEN) return null;
  const fields = readKernelFields(prn);
  if (fields === null) return null;
  // From here down is IAM's tenancy rule, deliberately outside the catch above.
  const { resourceType, resourceId: id, org } = fields;
  if (!TENANCY_KINDS.has(resourceType)) return null;
  const kind = resourceType as TenancyKind;
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

/**
 * IAM's principal PRN (a user or a service account): `resourceType === 'principal'`, no org field
 * — the same shape rule as `organization`. NOT a tenancy node: a principal is an actor, not a place
 * in the org/team/project tree, so it is not a `TenancyKind` and `parseTenancyPrn` never returns
 * one. Kept here because it reads the same kernel call as the tenancy readers above.
 */
export type PrincipalRef = { readonly id: string };

/** The principal a PRN names, or null for any other resource and for an invalid PRN. Never throws. */
export function parsePrincipalPrn(prn: string): PrincipalRef | null {
  if (prn.length === 0 || prn.length > MAX_LEN) return null;
  const fields = readKernelFields(prn);
  if (fields === null) return null;
  return fields.resourceType === 'principal' && fields.org === '' ? { id: fields.resourceId } : null;
}

export function principalPrn(id: string): string {
  return prnBuild('iam', '', '', 'principal', requireUuid('id', id));
}
