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

/** The callback was rejected before any token exchange. `reason` is a closed vocabulary. */
export class CallbackRejected extends AuthError {
  readonly code = 'callback_rejected';
  constructor(readonly reason: 'txn_missing' | 'txn_mismatch' | 'state_unknown' | 'code_exchange_failed') {
    super(`callback rejected: ${reason}`);
  }
}
