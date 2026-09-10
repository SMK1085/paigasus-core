// SPDX-License-Identifier: Apache-2.0
//
// The two total transport-status tables plus the transport-cause table (spec § 9.2). Internal:
// reached through ./errors, so it carries no guard of its own.
import { Code } from '@connectrpc/connect';

import type { Presentation, TransportCause } from './types.js';

/**
 * gRPC `Code` -> `Presentation`. Total by falling through to `generic`.
 *
 * `ResourceExhausted` sits under `degraded` rather than getting its own state: a gateway proxying
 * OpenAI produces it routinely, and it means "try later", which is what `degraded` renders.
 * `retryable` carries the finer signal. This row was reviewed and kept deliberately (spec § 9.2).
 */
const GRPC_TABLE: ReadonlyMap<Code, Presentation> = new Map([
  [Code.Unauthenticated, 'relogin'],
  [Code.PermissionDenied, 'forbidden'],
  [Code.NotFound, 'not-found'],
  [Code.Unavailable, 'degraded'],
  [Code.DeadlineExceeded, 'degraded'],
  [Code.ResourceExhausted, 'degraded'],
  [Code.InvalidArgument, 'invalid-input'],
  [Code.AlreadyExists, 'conflict'],
  [Code.FailedPrecondition, 'conflict'],
  [Code.Aborted, 'conflict'],
  [Code.Unimplemented, 'disabled'],
]);

/**
 * HTTP status -> `Presentation`. Total by falling through to `generic`.
 *
 * There is deliberately NO 200 row. A terminal SSE error frame carries status 200, and its
 * `degraded` presentation comes from the reason override table, not from here (spec § 9.4).
 */
const HTTP_TABLE: ReadonlyMap<number, Presentation> = new Map([
  [401, 'relogin'],
  [403, 'forbidden'],
  [404, 'not-found'],
  [408, 'degraded'],
  [429, 'degraded'],
  [502, 'degraded'],
  [503, 'degraded'],
  [504, 'degraded'],
  [400, 'invalid-input'],
  [413, 'invalid-input'],
  [415, 'invalid-input'],
  [422, 'invalid-input'],
  [409, 'conflict'],
  [501, 'disabled'],
]);

const CAUSE_TABLE: Readonly<Record<TransportCause, Presentation>> = {
  timeout: 'degraded',
  network: 'degraded',
  // A caller abort is not a service fault. Rendering it as `degraded` would blame the service
  // for the caller's own decision.
  aborted: 'generic',
};

export function presentationForGrpcCode(code: Code): Presentation {
  return GRPC_TABLE.get(code) ?? 'generic';
}

export function presentationForHttpStatus(status: number): Presentation {
  return HTTP_TABLE.get(status) ?? 'generic';
}

export function presentationForTransportCause(cause: TransportCause): Presentation {
  return CAUSE_TABLE[cause];
}

/** The `Code` member name, for logging. `Code[code]` is the reverse map a numeric enum carries. */
export function grpcCodeName(code: Code): string {
  return (Code[code] as string | undefined) ?? String(code);
}
