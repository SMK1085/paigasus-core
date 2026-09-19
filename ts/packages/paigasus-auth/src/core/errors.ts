// SPDX-License-Identifier: Apache-2.0

/** Base for every error this package raises. Carries no secret in its message, ever. */
export abstract class AuthError extends Error {
  abstract readonly code: string;
}

/** The store could not be reached. Callers treat this as "no session", never as a 500. */
export class SessionStoreUnavailable extends AuthError {
  readonly code = 'session_store_unavailable';
}

/** Which half of the deadline decorator refused the operation (SMA-651 D7). */
export type SessionStoreTimeoutPhase = 'deadline' | 'circuit-open';

/**
 * The store did not answer in time (SMA-651). `deadline` means this operation ran past its bound;
 * `circuit-open` means an earlier one did and this one was refused without touching the socket.
 *
 * A SUBCLASS of SessionStoreUnavailable, deliberately: every caller that classifies on that class
 * (http/store-unavailable.ts's storeStep, reached from every store call in http/routes.ts, and
 * next/get-session.ts) keeps working unchanged, and
 * `code` stays 'session_store_unavailable'. `name` is set explicitly because a production bundle
 * can mangle `constructor.name`. The message names one of seven fixed operation literals and a
 * number, never the DSN.
 *
 * NOT exported from src/server.ts, for the reason RefreshRejected records below: getSession()
 * swallows every failure, so no consumer can observe one.
 */
export class SessionStoreTimeout extends SessionStoreUnavailable {
  readonly phase: SessionStoreTimeoutPhase;

  constructor(operation: string, deadlineMs: number, phase: SessionStoreTimeoutPhase) {
    super(
      phase === 'deadline'
        ? `the session store operation "${operation}" did not answer within ${String(deadlineMs)} ms`
        : `the session store is not answering: "${operation}" was refused while the circuit was open`,
    );
    this.name = 'SessionStoreTimeout';
    this.phase = phase;
  }
}

/**
 * True when `err` is an `Error` whose own `code` property equals `code`, from ANY copy of this
 * module.
 *
 * Why not `instanceof`: Next 16 gives a route handler and a page SEPARATE copies of this package
 * (measured, see src/runtime.ts's comment on RUNTIME_KEY_PREFIX), and state is shared between them
 * through globalThis. An object built by one copy is therefore routinely classified by the other,
 * where `instanceof` against the local class is false. The `code` class field is an own property of
 * every instance, and a subclass inherits it, so it survives the duplication.
 *
 * THE RULE, stated once (SMA-657 D7). A class thrown by a closure that is reachable through shared
 * state, and caught OUTSIDE that closure, must be classified by its `code`. A class thrown and
 * caught inside one closure, or thrown and caught by two modules of one copy, may use `instanceof`
 * — the boundary is the CLOSURE, not whether state is shared. The three remaining `instanceof`
 * sites in this package each carry a comment saying which side of that line they fall on.
 *
 * The `err instanceof Error` test is an `instanceof` against a BUILTIN, which both copies share in
 * one isolate, so it does not have the defect this function exists to avoid. It is what rejects a
 * plain object that happens to carry a matching `code`.
 *
 * This is deliberately WIDER than `instanceof`: any `Error` carrying the code passes, not only an
 * instance of the declaring class. That widening IS the mechanism.
 */
function hasAuthErrorCode(err: unknown, code: string): boolean {
  return err instanceof Error && (err as { code?: unknown }).code === code;
}

/** True for a SessionStoreUnavailable, or a subclass such as SessionStoreTimeout, from ANY copy of
 * this module (SMA-653 D2). See `hasAuthErrorCode` for why this is not an `instanceof`. */
export function isSessionStoreUnavailable(err: unknown): boolean {
  return hasAuthErrorCode(err, 'session_store_unavailable');
}

/** True for a RefreshRejected from ANY copy of this module (SMA-657 D1). The crossing path is
 * real and not hypothetical: next/get-session.ts wires `refresh` to the shared `runtime.oidc`, so
 * adapters/oidc.ts builds this class in whichever copy created the runtime, and
 * core/single-flight.ts classifies it in whichever copy serves the request. See
 * `hasAuthErrorCode`. */
export function isRefreshRejected(err: unknown): boolean {
  return hasAuthErrorCode(err, 'oidc_refresh_rejected');
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
