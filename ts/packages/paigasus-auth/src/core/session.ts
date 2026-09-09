// SPDX-License-Identifier: Apache-2.0
import type { IdTokenClaims, ResolvedPrincipal, RoleGrantRef } from '../ports/principal-resolver.js';

export interface SessionRecord {
  /**
   * A mismatch on read is treated as ABSENT and the record is deleted. There is no migration
   * path by design; the alternative is deserialising into a wrong-typed object.
   * OPERATIONAL CONSEQUENCE: the deploy that bumps this logs out every active user. Say so in
   * the release note.
   */
  version: 1;
  /**
   * Fencing counter. `SessionStore.set` is a compare-and-set on it. Without this, a refresh that
   * outlives its lock TTL lets the slow holder write its now-revoked token over a newer valid
   * one — a lock's compare-and-delete protects the LOCK, never the WRITE.
   */
  rev: number;
  accessToken: string;
  refreshToken?: string;
  accessExpiresAt: number;
  /** loginTime + ABSOLUTE_TTL. Never extended by a refresh; a refresh clamps to it. */
  absoluteExpiresAt: number;
  idTokenClaims: IdTokenClaims;
  principal: ResolvedPrincipal;
}

/**
 * Everything /client may ever see. A runtime tuple, not just a type: an interface has no runtime
 * key set, so a strict-equality assertion needs something to compare against.
 */
export const SESSION_VIEW_KEYS = ['principalPrn', 'displayName', 'email', 'grants', 'grantsAvailable'] as const;

export type SessionViewKey = (typeof SESSION_VIEW_KEYS)[number];

export interface SessionView {
  principalPrn: string | null;
  displayName: string | null;
  email: string | null;
  grants: RoleGrantRef[];
  grantsAvailable: boolean;
}

/** The ONLY function that may construct what crosses to the browser. */
export function toSessionView(rec: SessionRecord): SessionView {
  const email = typeof rec.idTokenClaims.email === 'string' ? rec.idTokenClaims.email : null;
  const name = typeof rec.idTokenClaims.name === 'string' ? rec.idTokenClaims.name : null;
  return {
    principalPrn: rec.principal.principalPrn,
    displayName: name ?? (email === null ? null : (email.split('@')[0] ?? null)),
    email,
    grants: rec.principal.roleGrants,
    grantsAvailable: rec.principal.grantsAvailable,
  };
}
