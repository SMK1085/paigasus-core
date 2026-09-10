// SPDX-License-Identifier: Apache-2.0

/** Base for every error this package raises. Carries no secret in its message, ever. */
export abstract class AuthError extends Error {
  abstract readonly code: string;
}

/** The store could not be reached. Callers treat this as "no session", never as a 500. */
export class SessionStoreUnavailable extends AuthError {
  readonly code = 'session_store_unavailable';
}

/** Configuration is internally inconsistent. Thrown by createAuthRuntime at first request. */
export class AuthConfigError extends AuthError {
  readonly code = 'auth_config_invalid';
}

/**
 * The callback was rejected before any token exchange. `reason` is a closed vocabulary.
 *
 * `idp_error` and `code_exchange_failed` are deliberately distinct (review round 1, Important 1):
 * `idp_error` is the identity provider itself declining (e.g. the user clicked Cancel) — a benign,
 * expected outcome carrying its own `?error=` query parameter, never a token-endpoint call.
 * `code_exchange_failed` is reserved for a genuine failure of the exchange itself (an
 * unreachable/erroring token endpoint, a rejected code). Conflating the two would log a user
 * cancelling identically to an outage.
 */
export class CallbackRejected extends AuthError {
  readonly code = 'callback_rejected';
  constructor(readonly reason: 'txn_missing' | 'txn_mismatch' | 'state_unknown' | 'idp_error' | 'code_exchange_failed') {
    super(`callback rejected: ${reason}`);
  }
}

/**
 * The identity provider REFUSED this refresh token, as opposed to failing to answer. That
 * distinction decides whether the user is signed out or kept on a still-live access token, so it
 * is a core concept, not an adapter detail — adapters/oidc.ts maps its library's error onto this,
 * which is what lets core/single-flight.ts classify without importing adapters/ (SMA-626 § 2.3).
 *
 * `oauthError` is typed as the CLOSED set this package admits, not `string`. RFC 6749 § 5.2 does
 * not bound the value — it is whatever the server's body carried — so a `string` here would leave
 * the redaction claim resting on the call site, and a later edit widening the set would silently
 * widen what may be logged. The type and the classifier's membership test are one fact.
 *
 * NOT exported from src/server.ts, deliberately: no consumer can produce or observe one.
 * getSession() swallows every failure into `null`, and CreateAuthRuntimeDeps exposes no OIDC
 * override.
 */
export class RefreshRejected extends AuthError {
  readonly code = 'oidc_refresh_rejected';
  constructor(readonly oauthError: 'invalid_grant') {
    super(`oidc refresh rejected: ${oauthError}`);
  }
}
