// SPDX-License-Identifier: Apache-2.0
//
// The ONE reading of IAM's `principal_prn` (review, defect 1).
//
// THE DRIFT THIS CLOSES. Introspect answers a proto3 string, and an unset string arrives as `''`.
// Two call sites map that answer and they disagreed: the login resolver (lib/principal-resolver.ts)
// normalised `'' -> null`, while the LIVE path (lib/principal.ts, which every page uses) passed the
// empty string through. An empty PRN then slipped past every `=== null` guard downstream: mayI()
// asked IAM `isAuthorized({ principalPrn: '' })`, IAM refused with InvalidArgument, mayI() failed
// open, and EVERY mutation control rendered for a principal IAM could not name — while myScopes()
// lost its grants to a listRoleGrants('') that failed for the same reason. Both paths read the
// field here now, so they cannot disagree about what an unnamed principal looks like again.
import 'server-only';

/**
 * IAM's principal PRN, or `null` when IAM named no principal. The value is returned VERBATIM when
 * it is a name at all: only this module decides what "no name" means, never what a name looks like
 * — IAM parses the PRN, and the console does not second-guess its grammar (spec § 6.1). Whitespace
 * alone is not a name either, so it reads as `null` rather than reaching IAM as a PRN.
 */
export function principalPrnOf(raw: string): string | null {
  return raw.trim() === '' ? null : raw;
}
