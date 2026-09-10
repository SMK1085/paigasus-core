// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { Code } from '@connectrpc/connect';

import type { Presentation } from './types.js';

/**
 * gRPC Code -> Presentation (spec § 9.2). Total: everything unlisted is `generic`.
 *
 * `Unimplemented` is deliberately ABSENT, so it falls through to `generic`. It means "this build
 * cannot do that"; "this deployment turned that capability off" is a different screen, and
 * § 9.4's CAPABILITY_DISABLED entry is what selects it. Mapping it to `disabled` here would make
 * that entry a no-op.
 *
 * `ResourceExhausted` is listed although nothing emits it today (spec M13). The table enumerates
 * `Code`, and this is the right answer if IAM ever adds a quota — it is not evidence that
 * something produces it.
 */
const GRPC: ReadonlyMap<Code, Presentation> = new Map([
  [Code.Unauthenticated, 'relogin'],
  [Code.PermissionDenied, 'forbidden'],
  [Code.NotFound, 'not-found'],
  [Code.Unavailable, 'degraded'],
  [Code.DeadlineExceeded, 'degraded'],
  [Code.InvalidArgument, 'invalid-input'],
  [Code.AlreadyExists, 'conflict'],
  [Code.FailedPrecondition, 'conflict'],
  [Code.Aborted, 'conflict'],
  [Code.ResourceExhausted, 'rate-limited'],
]);

/**
 * HTTP status -> Presentation (spec § 9.2). Total: everything unlisted is `generic`.
 *
 * 429 is `rate-limited`, not `degraded`: a quota exhaustion and a sick service want different
 * copy. 504 stays `degraded` — a timeout is a service problem, not a quota one.
 *
 * 413 sits with 400 under `invalid-input` because a too-large request is caller-fixable: the
 * caller must send less, the same corrective action a 400 asks for.
 */
const HTTP: ReadonlyMap<number, Presentation> = new Map([
  [400, 'invalid-input'],
  [401, 'relogin'],
  [403, 'forbidden'],
  [404, 'not-found'],
  [408, 'degraded'],
  [409, 'conflict'],
  [413, 'invalid-input'],
  [415, 'invalid-input'],
  [422, 'invalid-input'],
  [429, 'rate-limited'],
  [502, 'degraded'],
  [503, 'degraded'],
  [504, 'degraded'],
]);

export function presentationForGrpcCode(code: Code): Presentation {
  return GRPC.get(code) ?? 'generic';
}

export function presentationForHttpStatus(status: number): Presentation {
  return HTTP.get(status) ?? 'generic';
}
