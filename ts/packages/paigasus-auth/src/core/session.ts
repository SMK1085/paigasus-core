// SPDX-License-Identifier: Apache-2.0
//
// SessionView, SESSION_VIEW_KEYS, and SessionViewKey are DEFINED in ../session-view.ts, not here,
// and re-exported below so existing importers of this module keep working. That module holds no
// server machinery, which is what lets src/client.ts import it directly; this file (and anything
// under core/**, adapters/**, ports/**) is banned from the client boundary by eslint.mjs's
// paigasus/boundaries/auth-client rule, including for type-only imports — moving the shared
// vocabulary out of core/ closes the gap rather than carving an exception into that rule.
import type { IdTokenClaims, ResolvedPrincipal } from '../ports/principal-resolver';
import { SESSION_VIEW_KEYS, type SessionView, type SessionViewKey } from '../session-view';

export { SESSION_VIEW_KEYS };
export type { SessionView, SessionViewKey };

export interface SessionRecord {
  /**
   * A mismatch on read is treated as ABSENT and the record is deleted. There is no migration
   * path by design; the alternative is deserialising into a wrong-typed object.
   * OPERATIONAL CONSEQUENCE: the deploy that bumps this logs out every active user. Say so in
   * the release note.
   *
   * Version 2 (SMA-681) added the required `idToken`. A bump has three more costs. During a rolling
   * update, old and new pods read each other's records as absent and delete them. A user can then
   * see a login loop until the rollout ends. A rollback forces a second logout. Both zones default
   * their image tag to `.Chart.AppVersion` (charts/paigasus/values.yaml), so the mixed state lasts
   * only for the rollout. After the deploy, a logout or any other read of a version 1 record
   * deletes it before its refresh token can be revoked. http/routes.ts reads no token from it.
   * That refresh token stays valid at the IdP until its idle timeout. Under the default scope
   * (`offline_access`, config.ts) it is an offline token. No deployment existed on 2026-09-25
   * (spec § 4.1).
   */
  version: 2;
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
  /**
   * The raw, signed ID token JWT: the newest one the IdP issued for this session (SMA-681). The
   * login sets it. A refresh replaces it only when the new token has the same `iss` and `sub` as
   * `idTokenClaims` (core/single-flight.ts). Only logout reads it, as `id_token_hint`. So after a
   * refresh it can come from a DIFFERENT token than `idTokenClaims`. `toSessionView` does not
   * read it, so it never crosses to the browser.
   */
  idToken: string;
  /**
   * The DECODED claims of the LOGIN ID token. A refresh does not change them (SMA-681 spec § 4.1).
   * The principal and the display name read these, never `idToken`.
   */
  idTokenClaims: IdTokenClaims;
  principal: ResolvedPrincipal;
}

/** The ONLY function that may construct what crosses to the browser. */
export function toSessionView(rec: SessionRecord): SessionView {
  const email = typeof rec.idTokenClaims.email === 'string' ? rec.idTokenClaims.email : null;
  const name = typeof rec.idTokenClaims.name === 'string' ? rec.idTokenClaims.name : null;
  return {
    principalPrn: rec.principal.principalPrn,
    displayName: name ?? (email === null ? null : (email.split('@')[0] ?? null)),
    email,
    // A DEEP COPY, not `rec.principal.roleGrants` by reference. The record crosses the JSON
    // boundary in every real caller, so aliasing was harmless there — but a client surface
    // (src/client.ts) now holds this array in-process, and a caller mutating the returned view
    // must never reach back into the SessionRecord still held by core/single-flight.ts.
    // `RoleGrantRef` is a flat `{ scopePrn, roleKey }` with no nested objects, so a shallow spread
    // of each element is a genuine deep copy here — a shallow `[...roleGrants]` alone still hands
    // out the same grant OBJECTS by reference (review round 1: `v.grants[0].roleKey = 'x'` reached
    // back into the record), which is exactly the hazard this copy exists to close.
    grants: rec.principal.roleGrants.map((grant) => ({ ...grant })),
    grantsAvailable: rec.principal.grantsAvailable,
  };
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * The ONE statement of what a stored session record must look like (SMA-626 § 4.2). Both adapters
 * call it, replacing the standalone `version !== 1` check each of them used to carry, so the
 * absent-and-deleted policy is stated once rather than twice.
 *
 * IT CHECKS EVERY REQUIRED FIELD, not only the ones the read path touches. Three measured holes
 * are the reason:
 *
 *   1. `JSON.parse('null')` SUCCEEDS, so redis-store's parse guard never fires on a stored
 *      literal `null`; reading `.version` off it then throws a TypeError that #guarded converts
 *      into SessionStoreUnavailable — a store-outage signal against a healthy Redis.
 *   2. A body of `{ version: 1 }` (the version then; `{ version: 2 }` today) passes two NaN
 *      comparisons in a row in resolveSession
 *      (`now >= undefined` and `now >= NaN` are both false), so it is returned as a LIVE session
 *      carrying `accessToken: undefined`.
 *   3. A principal shaped `{ roleGrants: [] }` yields `grantsAvailable: undefined` through
 *      toSessionView, and `can()` FAILS OPEN on that field (src/client.ts:67) — so every
 *      browser-side capability check returns true. The fail-open is correct and deliberate; this
 *      predicate is what keeps a poisoned record from reaching it.
 *
 * `refreshToken` is the one optional field: absent is legal, an explicit `null` is not
 * (`exactOptionalPropertyTypes`). `idToken` is required and must not be empty: logout sends it as
 * `id_token_hint` (SMA-681).
 *
 * THIS PREDICATE GATES READS, NOT WRITES. Both store adapters call it only in `get`, never in
 * `set`. A record that fails it could in principle still be WRITTEN — logged as
 * `session.created`, then deleted on its own first read with no event that explains why, a login
 * loop with no visible cause. In this package today that gap is unreachable: every write goes
 * through `handleCallback` or a refresh, both of which build the record from `ResolvedPrincipal`
 * and `OidcTokens` (typed, not `unknown`), and `CreateAuthRuntimeDeps.resolver` is checked by the
 * compiler, not at runtime. This is defence-in-depth against a future write path, not a live bug.
 */
export function isSessionRecord(value: unknown): value is SessionRecord {
  if (!isObject(value)) return false;
  if (value['version'] !== 2) return false;
  if (!Number.isFinite(value['rev'])) return false;
  if (!Number.isFinite(value['accessExpiresAt'])) return false;
  if (!Number.isFinite(value['absoluteExpiresAt'])) return false;
  if (typeof value['accessToken'] !== 'string') return false;
  if ('refreshToken' in value && typeof value['refreshToken'] !== 'string') return false;

  const idToken = value['idToken'];
  if (typeof idToken !== 'string' || idToken.length === 0) return false;

  const claims = value['idTokenClaims'];
  if (!isObject(claims)) return false;
  if (typeof claims['iss'] !== 'string' || typeof claims['sub'] !== 'string') return false;

  const principal = value['principal'];
  if (!isObject(principal)) return false;
  if (principal['principalPrn'] !== null && typeof principal['principalPrn'] !== 'string') return false;
  if (typeof principal['issuer'] !== 'string' || typeof principal['subject'] !== 'string') return false;
  if (typeof principal['grantsAvailable'] !== 'boolean') return false;
  const memberships = principal['memberships'];
  if (!Array.isArray(memberships)) return false;
  if (!memberships.every((m) => isObject(m) && typeof m['id'] === 'string' && typeof m['principalPrn'] === 'string' && typeof m['nodePrn'] === 'string')) return false;
  const grants = principal['roleGrants'];
  if (!Array.isArray(grants)) return false;
  return grants.every((g) => isObject(g) && typeof g['scopePrn'] === 'string' && typeof g['roleKey'] === 'string');
}
