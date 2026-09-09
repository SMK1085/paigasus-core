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
