// SPDX-License-Identifier: Apache-2.0
//
// The guard-free, client-safe surface (spec § 6.3, § 9.6). This file carries NO
// `import './server-guard.js'` and must not gain one: `tests/server-guard.test.ts` lists
// './errors/types' in UNGUARDED_ENTRIES and asserts the guard is absent.
//
// `Code` is imported as a TYPE only. Under `verbatimModuleSyntax` (ts/tsconfig.base.json:9) an
// `import type` emits nothing, so naming it here puts no runtime dependency on
// @connectrpc/connect into a client bundle.
import type { Code } from '@connectrpc/connect';
// Imported as TYPES for the field declarations below, and separately RE-EXPORTED as values.
// A re-export does not create a local binding, so both lines are needed and they do not conflict.
import type { ErrorDomain, ErrorReason } from '@paigasus/proto';

// The two registry enums, re-exported as VALUES. An app cannot import @paigasus/proto — the
// eslint boundary `paigasus/boundaries/apps` bans it, type imports included — so without this
// line a consumer receives `reason: 906` and cannot write the name. Two small frozen enum
// objects in the client bundle is the deliberate cost (spec § 9.6).
export { ErrorDomain, ErrorReason } from '@paigasus/proto';

/** What a consumer's error boundary switches on. Never the message, never the raw status. */
export type Presentation = 'relogin' | 'forbidden' | 'not-found' | 'degraded' | 'invalid-input' | 'conflict' | 'disabled' | 'generic';

/** Why a request produced no response at all. */
export type TransportCause = 'timeout' | 'network' | 'aborted';

/**
 * The raw transport outcome, carried for LOGGING only. A consumer branches on `presentation`.
 *
 * `codeName` sits beside `code` so a reader can render the name without a VALUE import of
 * @connectrpc/connect, which is a server-side dependency (spec § 9.1).
 */
export type TransportInfo =
  | { readonly kind: 'grpc'; readonly code: Code; readonly codeName: string }
  | { readonly kind: 'http'; readonly status: number }
  | { readonly kind: 'transport'; readonly cause: TransportCause };

/**
 * One shape for every failure the SDK can report.
 *
 * It is a PLAIN OBJECT, never an `Error` subclass, because React's server-to-client serializer
 * rejects class instances and this object crosses that boundary as a prop (spec § 6.3, § 9.1).
 *
 * `message` is for display and logging and is NEVER an input to a branch (AC 1). It holds no
 * `ConnectError` and no `Headers`, which is what AC 3 requires.
 */
export interface PaigasusError {
  readonly presentation: Presentation;
  /** `null` when the wire's domain did not resolve; `rawDomain` still holds what it said. */
  readonly domain: ErrorDomain | null;
  /** `null` when the wire's reason did not resolve; `rawReason` still holds what it said. */
  readonly reason: ErrorReason | null;
  readonly rawReason: string | null;
  readonly rawDomain: string | null;
  readonly message: string;
  readonly correlationId: string | null;
  readonly requestId: string | null;
  /** Tri-state. `null` is the wire's "unknown"; collapsing it to `false` would assert what the service declined to assert (ADR-0019 decision 7). */
  readonly retryable: boolean | null;
  readonly metadata: Readonly<Record<string, string>>;
  readonly transport: TransportInfo;
}
