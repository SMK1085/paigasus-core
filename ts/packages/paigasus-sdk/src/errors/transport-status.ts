// SPDX-License-Identifier: Apache-2.0
//
// The two total transport-status tables plus the transport-cause table (spec § 9.2). Internal:
// reached through ./errors, so it carries no guard of its own.
import { Code } from '@connectrpc/connect';

import type { Presentation, TransportCause } from './types';

/**
 * gRPC `Code` -> `Presentation`. Total by falling through to `generic`.
 *
 * Two rows carry a decision rather than an obvious mapping.
 *
 * `ResourceExhausted` gets its OWN `rate-limited` state rather than sharing `degraded`. A quota
 * refusal and a sick service want different copy. MEASURED: nothing in this repository emits 429
 * or `ResourceExhausted` today, so every one the SDK can currently see is the UPSTREAM's quota
 * arriving through the chat passthrough — which is exactly what lets that copy be specific instead
 * of hedged. A future Paigasus-side quota gets a registry reason, and the override table can give
 * it this same screen without a tenth presentation value.
 *
 * `Unimplemented` maps to `generic`, NOT to `disabled`, and that is load-bearing. IAM's
 * `capability_disabled` is the only thing that emits `Unimplemented` (`convert.rs:96-104`), so if
 * this row said `disabled` the `CAPABILITY_DISABLED` OVERRIDE would produce the answer this table
 * already gave and the whole override mechanism would be decoration on that reason. Sending
 * `Unimplemented` to `generic` and letting the override lift it to `disabled` keeps the two
 * meanings apart: "this build cannot do that" against "this deployment turned that capability
 * off" (spec § 9.2, § 9.4).
 */
const GRPC_TABLE: ReadonlyMap<Code, Presentation> = new Map([
  [Code.Unauthenticated, 'relogin'],
  [Code.PermissionDenied, 'forbidden'],
  [Code.NotFound, 'not-found'],
  [Code.Unavailable, 'degraded'],
  [Code.DeadlineExceeded, 'degraded'],
  [Code.ResourceExhausted, 'rate-limited'],
  [Code.InvalidArgument, 'invalid-input'],
  [Code.AlreadyExists, 'conflict'],
  [Code.FailedPrecondition, 'conflict'],
  [Code.Aborted, 'conflict'],
  [Code.Unimplemented, 'generic'],
]);

/**
 * HTTP status -> `Presentation`. Total by falling through to `generic`.
 *
 * There is deliberately no 501 row either. Nothing in this repository emits 501 — the gateway's
 * `StreamingDisabled` answers 400 (`error.rs:170`) and `Unimplemented` has no HTTP form at all — so
 * a 501 could only come from an ingress or a proxy, where "the server does not support this
 * method" is the same "this build cannot do that" meaning `Unimplemented` now carries. It falls
 * through to `generic` with them.
 *
 * There is deliberately NO 200 row. A terminal SSE error frame carries status 200, and its
 * `degraded` presentation comes from the reason override table, not from here (spec § 9.4).
 */
const HTTP_TABLE: ReadonlyMap<number, Presentation> = new Map([
  [401, 'relogin'],
  [403, 'forbidden'],
  [404, 'not-found'],
  [408, 'degraded'],
  [429, 'rate-limited'],
  [502, 'degraded'],
  [503, 'degraded'],
  [504, 'degraded'],
  [400, 'invalid-input'],
  [413, 'invalid-input'],
  [415, 'invalid-input'],
  [422, 'invalid-input'],
  [409, 'conflict'],
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
  return Code[code] ?? String(code);
}
