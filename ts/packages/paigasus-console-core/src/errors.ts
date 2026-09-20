// SPDX-License-Identifier: Apache-2.0
//
// The ONE wrapper around an SDK call (spec § 6.1). It turns a ConnectError into the SDK's
// PaigasusError and returns it as data. It RETHROWS EVERY OTHER ERROR UNCHANGED: Next's redirect(),
// notFound() and forbidden() are thrown errors, and so is a real bug; swallowing either would turn
// a navigation into an error page or hide the bug. The app branches on presentation, domain and
// reason only, never on message text (ADR-0019 E8).
//
// MODULE COPIES, AND WHY THE CHECK BELOW IS SAFE (SMA-662). Next gives a route handler and a page SEPARATE
// module graphs, so this package exists twice in one process and a class has two identities. The
// `instanceof` below survives that, and the reason lives in the DEPENDENCY, not here:
// `ConnectError` declares a static Symbol.hasInstance that falls back to a duck-type BRAND —
// `name === 'ConnectError'` plus `code`, `metadata`, `details`, `rawMessage` and `cause` —
// whenever the prototype does not match (@connectrpc/connect 2.2.0,
// dist/esm/connect-error.js:82-98). An instance the other copy built passes every clause.
// tests/unit/call-iam.test.ts pins that contract, because nothing else here would notice a
// connect-es release dropping it. Note the brand is WIDER than a prototype test: any Error of that
// shape is admitted, and that widening IS the mechanism.
//
// WHAT CROSSES AND WHAT DOES NOT. createConsoleRuntime builds every product fresh per call and
// reads no globalThis. Three things are shared: the descriptor cache and its Redis client, one per
// process (discovery.ts's globalThis symbol); the AuthRuntime, one per process, owned by
// @paigasus/auth; and @paigasus/sdk's transport cache, one per module COPY. The IAM clients
// themselves never cross. The full audit, and the rule it applies (SMA-657 D7), are in
// docs/superpowers/specs/2026-09-20-sma-662-console-core-instanceof-audit-design.md.
import 'server-only';
import { ConnectError } from '@connectrpc/connect';
import { mapError, type PaigasusError } from '@paigasus/sdk/errors';
import { requestCorrelationId, requestPath } from './correlation';
import { logger } from './logger';

export type IamResult<T> = { ok: true; value: T } | { ok: false; error: PaigasusError };

/** What a Server Action returns to its form. PaigasusError is a plain object, so it crosses Flight. */
export type ActionState = { ok: true } | { ok: false; error: PaigasusError } | null;

export async function callIam<T>(fn: () => Promise<T>): Promise<IamResult<T>> {
  try {
    return { ok: true, value: await fn() };
  } catch (err) {
    // Safe across the two module copies: ConnectError's Symbol.hasInstance brand admits an
    // instance the other copy built, so this is not a prototype test (see the header).
    if (!(err instanceof ConnectError)) throw err;
    const error = mapError({ kind: 'grpc', error: err });
    // One line per failed call. It carries the correlation id IAM put into the error AND the one
    // proxy.ts minted for this request, so an operator can join the console log to IAM's even when
    // the 403 view shows no id (the spec § 6.2 fallback).
    const [requestCorrelation, path] = await Promise.all([requestCorrelationId(), requestPath()]);
    logger.appEvent('iam.call_failed', {
      presentation: error.presentation,
      reason: error.rawReason,
      code: error.transport.kind === 'grpc' ? error.transport.codeName : null,
      correlation_id: error.correlationId,
      request_correlation_id: requestCorrelation,
      path,
    });
    return { ok: false, error };
  }
}

/**
 * Fills every `PaigasusError` field that is the same for an error that never reached IAM: no
 * domain, no reason, no correlation id. The caller gives only the fields that are specific to its
 * own case, so a new shared field is a single edit here instead of one in every caller.
 */
export function neverReachedIam(fields: Pick<PaigasusError, 'presentation' | 'message' | 'transport'>): PaigasusError {
  return {
    ...fields,
    domain: null,
    reason: null,
    rawReason: null,
    rawDomain: null,
    correlationId: null,
    requestId: null,
    retryable: false,
    metadata: {},
  };
}

/**
 * The session ended before the user submitted the form. Like a form the app's zod refused, this
 * error never reached IAM (see `neverReachedIam`).
 *
 * `presentation: 'relogin'` makes `FormError` render the `SignInAgain` LINK (spec § 6.4: relogin is
 * a link, never an automatic redirect). That link is what keeps the browser inside the zone — see
 * `runtime.ts`'s `iamClientsForAction` for why a Server Action must not redirect here.
 */
export function sessionExpired(): PaigasusError {
  return neverReachedIam({ presentation: 'relogin', message: 'The session has ended.', transport: { kind: 'http', status: 401 } });
}
