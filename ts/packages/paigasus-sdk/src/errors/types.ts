// SPDX-License-Identifier: Apache-2.0
//
// The client-safe surface (spec § 6.3). This file carries NO server guard, and that is the one
// deliberate exception in the package: it holds only types, and `verbatimModuleSyntax: true`
// (ts/tsconfig.base.json:9) means an `import type` emits nothing at all. A 'use client' error
// boundary can therefore name PaigasusError and switch on Presentation with no runtime import and
// no server-only evaluation.
//
// `tests/server-guard.test.ts` asserts this file does NOT import the guard — the assertion is
// pre-registered in its UNGUARDED_ENTRIES set, so removing the exception is a test failure.
import type { Code } from '@connectrpc/connect';
import type { ErrorDomain, ErrorReason } from '@paigasus/proto';

/**
 * What screen this error should produce.
 *
 * Nine values. `rate-limited` is separate from `degraded` deliberately: "you are being
 * rate-limited, try again shortly" and "the service is unwell" are different screens, and lumping
 * them costs the first the only copy that helps a user act (spec § 9.2).
 */
export type Presentation = 'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'rate-limited' | 'invalid-input' | 'conflict' | 'disabled' | 'generic';

/**
 * The raw transport status, carried for logging only.
 *
 * A number or a `Code` — NEVER a `ConnectError` and never `Headers`. That is what makes AC 4's
 * "raw gRPC statuses never reach the browser" hold when the whole object is serialized to a client
 * component as a prop.
 */
export type ErrorTransport = { readonly kind: 'grpc'; readonly code: Code } | { readonly kind: 'http'; readonly status: number };

/**
 * One shape for every failure this SDK can surface (spec § 9.1).
 *
 * Every optional field is `null`, never `undefined`. The codec in `@paigasus/proto` returns
 * `undefined`, so each call site normalizes with `?? null`: this object crosses the server/client
 * boundary as a prop, and `null` survives that uniformly while `undefined` does not.
 */
export interface PaigasusError {
  readonly presentation: Presentation;
  /**
   * Resolved only on the gRPC arm. IAM's HTTP envelope is `{error:{code,message}}` and the
   * gateway's is `{message,type,param,code}` — neither has a domain field — so this is
   * structurally `null` for arms 2, 3 and 4 (spec § 9.1).
   */
  readonly domain: ErrorDomain | null;
  /** `null` means the wire's code did not resolve against the registry. `rawReason` still holds it. */
  readonly reason: ErrorReason | null;
  readonly rawReason: string | null;
  readonly rawDomain: string | null;
  /** Human-readable, for display and logging. NEVER an input to a branch (AC 2). */
  readonly message: string;
  readonly correlationId: string | null;
  readonly requestId: string | null;
  /** Tri-state. `null` is the wire's "unknown" — never collapse it to `false` (ADR-0019 decision 7). */
  readonly retryable: boolean | null;
  /** The ErrorInfo map minus the three lifted keys (`retryable`, `correlation_id`, `request_id`). */
  readonly metadata: Readonly<Record<string, string>>;
  readonly transport: ErrorTransport;
}
