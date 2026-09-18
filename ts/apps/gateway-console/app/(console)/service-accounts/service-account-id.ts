// SPDX-License-Identifier: Apache-2.0
//
// A service account is a principal, and IAM names it `prn:pgs:iam:::principal/<uuid>`
// (rs/crates/libs/paigasus-iam-core/src/value.rs). The `sa` search parameter carries the UUID only
// (SMA-636 D14). Client-safe: no import at all, so the result region can build a select link.
//
// The file name is deliberate: a base name `prn` is a Windows reserved device name (CLAUDE.md).

const PRINCIPAL_PREFIX = 'prn:pgs:iam:::principal/';
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export function serviceAccountPrn(id: string): string {
  return `${PRINCIPAL_PREFIX}${id.toLowerCase()}`;
}

/** The UUID of a principal PRN, lower case, or null for any other text. */
export function serviceAccountIdOf(prn: string): string | null {
  if (!prn.startsWith(PRINCIPAL_PREFIX)) return null;
  const id = prn.slice(PRINCIPAL_PREFIX.length);
  return UUID_RE.test(id) ? id.toLowerCase() : null;
}

/** `?sa=`: a UUID selects one account. Any other value is ignored, in the parseOffset pattern (§ 4.1). */
export function parseAccountParam(raw: string | readonly string[] | undefined): string | null {
  const value = typeof raw === 'string' ? raw : raw?.[0];
  return value !== undefined && UUID_RE.test(value) ? value.toLowerCase() : null;
}
