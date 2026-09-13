// SPDX-License-Identifier: Apache-2.0
//
// The ONE wrapper around an SDK call (spec § 6.1). It turns a ConnectError into the SDK's
// PaigasusError and returns it as data. It RETHROWS EVERY OTHER ERROR UNCHANGED: Next's redirect(),
// notFound() and forbidden() are thrown errors, and so is a real bug; swallowing either would turn
// a navigation into an error page or hide the bug. The app branches on presentation, domain and
// reason only, never on message text (ADR-0019 E8).
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
