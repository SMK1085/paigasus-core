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
 * True when `err` is an `Error` whose `code` property equals `code`, from ANY copy of this
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
 * — the boundary is the CLOSURE, not whether state is shared. The four remaining sites in this
 * package that test a NON-BUILTIN class each carry a comment saying which side of that line they
 * fall on: server.ts's CallbackRejected catch, adapters/operation-deadline.ts's SessionStoreTimeout
 * check, and adapters/oidc.ts's classifyRefreshError and classifyDiscoveryError (SMA-656). All four
 * are on the `instanceof` side.
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

/** Why OIDC discovery failed (SMA-656 D8). The one list; the type derives from it. */
export const OIDC_DISCOVERY_FAILURE_REASONS = ['timeout', 'network', 'dns', 'tls', 'http_server_error', 'http_client_error', 'invalid_metadata', 'issuer_mismatch', 'other'] as const;
export type OidcDiscoveryFailureReason = (typeof OIDC_DISCOVERY_FAILURE_REASONS)[number];

/**
 * OIDC discovery did not complete (SMA-656). The message holds only the library error's name, and
 * there is no `cause` (D3): the `cause` of an issuer-mismatch ClientError holds the expected issuer
 * and the metadata, the `cause` of a 404 ClientError is a Response with a `url`, and Node prints the
 * `cause` chain when it logs an error. `reason` comes from adapters/oidc.ts's
 * `classifyDiscoveryError` (D8).
 *
 * INVARIANT (D10): thrown only by the adapter's `getConfig()`, which every method awaits before any
 * other request — so when a caller sees this, no token request was sent, no authorization code was
 * spent and no token exists. http/routes.ts's callback 503 depends on this: it does not revoke.
 *
 * `name` is set explicitly for the reason SessionStoreTimeout records: a production bundle can
 * mangle `constructor.name`. NOT exported from src/server.ts, for the SMA-657 D4 reason: a caller
 * of createOidcClient or AuthRuntime.oidc CAN receive one, but no consumer needs to CLASSIFY one.
 */
export class OidcDiscoveryFailed extends AuthError {
  readonly code = 'oidc_discovery_failed';
  readonly reason: OidcDiscoveryFailureReason;

  constructor(message: string, reason: OidcDiscoveryFailureReason) {
    super(message);
    this.name = 'OidcDiscoveryFailed';
    this.reason = reason;
  }
}

/** True for an OidcDiscoveryFailed from ANY copy of this module (SMA-656 D1). The crossing path is
 * real: the runtime and its `oidc` client are shared through globalThis, so adapters/oidc.ts builds
 * this class in whichever copy created the runtime, and http/routes.ts classifies it in whichever
 * copy serves the request. See `hasAuthErrorCode`. */
export function isOidcDiscoveryFailed(err: unknown): boolean {
  return hasAuthErrorCode(err, 'oidc_discovery_failed');
}

/**
 * The `reason` of a caught discovery error, read defensively (SMA-656 D8). A value outside
 * OIDC_DISCOVERY_FAILURE_REASONS — a missing field, a foreign string, a number — gives 'other', so
 * no value that the list does not name can reach a log line.
 */
export function oidcDiscoveryReason(err: unknown): OidcDiscoveryFailureReason {
  const reason: unknown = typeof err === 'object' && err !== null ? (err as { reason?: unknown }).reason : undefined;
  return OIDC_DISCOVERY_FAILURE_REASONS.find((known) => known === reason) ?? 'other';
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
