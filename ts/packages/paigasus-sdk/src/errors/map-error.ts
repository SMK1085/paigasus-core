// SPDX-License-Identifier: Apache-2.0
//
// The five-arm normalizer (spec § 9.5). Internal: reached through ./errors.
//
// It branches on (domain, reason) and on the transport status. It NEVER reads `message` to decide
// anything — that is AC 1, and it is satisfied structurally by there being no such read here.
import { ConnectError } from '@connectrpc/connect';
// `type ErrorReason` inline rather than a second import statement: verbatimModuleSyntax is on, so
// a type in a value import must be marked, and one statement per module keeps the lint quiet.
import { type ErrorReason, ErrorInfoSchema, fromWireDomain, fromWireReason } from '@paigasus/proto';

import { presentationOverride } from './presentation';
import { grpcCodeName, presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from './transport-status';
import type { PaigasusError, Presentation, TransportCause, TransportInfo } from './types';

/**
 * The correlation and request ids read off a committed response head.
 *
 * A mid-stream failure is the ONE case where the terminal frame carries no ids of its own, and it
 * is also the case a user is most likely to report. The head had them; the caller passes them
 * back in rather than losing them.
 */
export type FrameIds = { readonly correlationId: string | null; readonly requestId: string | null };

/** The three keys lifted out of ErrorInfo.metadata into their own typed fields. */
const LIFTED_KEYS = ['retryable', 'correlation_id', 'request_id'] as const;

/**
 * Exported so `src/chat.ts` reads the two success-arm ids off the same literals this module maps
 * an error with, rather than a second copy of the header names that could drift from these.
 */
export const CORRELATION_HEADER = 'paigasus-correlation-id';
export const REQUEST_ID_HEADER = 'paigasus-request-id';
const RETRYABLE_HEADER = 'paigasus-retryable';

export type ErrorInput =
  | { readonly kind: 'grpc'; readonly error: ConnectError }
  /**
   * Both HTTP envelopes. IAM sends `{error:{code,message}}` and the gateway sends
   * `{error:{message,type,param,code}}`; as DATA they are one envelope plus an optional `param`,
   * so one reader handles both. `body` is `unknown` because it may be an unparsed non-JSON body.
   */
  | { readonly kind: 'http'; readonly status: number; readonly headers: Headers; readonly body: unknown }
  /**
   * A parsed terminal SSE frame. `status` is the status the gateway actually COMMITTED on the
   * response head, and `ids` are the correlation and request ids that head carried — the parser
   * sees only bytes and can know neither. Ported from PR #231, whose review caught that a
   * hardcoded 200 misreports a stream committed on any other 2xx.
   */
  | { readonly kind: 'terminal-frame'; readonly body: unknown; readonly status: number; readonly ids?: FrameIds | undefined }
  | { readonly kind: 'transport'; readonly cause: TransportCause; readonly message: string };

/** The wire's tri-state. `unknown`, an absent value, and anything unrecognized all mean `null`. */
function parseRetryable(value: string | null | undefined): boolean | null {
  if (value === 'true') return true;
  if (value === 'false') return false;
  return null;
}

/**
 * The first value that is a non-empty string, else `null`. An empty id is no id.
 *
 * EXPORTED because `chat.ts` reads the same two headers onto `ChatResult`. Use it at EVERY id read,
 * not only where a fallback chain exists: `''` is an invalid value for a field whose `null` means
 * "the wire carried no id", independently of whether anything follows it.
 */
export function firstNonEmpty(...values: readonly (string | null | undefined)[]): string | null {
  for (const value of values) {
    if (typeof value === 'string' && value !== '') return value;
  }
  return null;
}

/** `undefined` is what the codec returns; `null` is what PaigasusError carries. */
function orNull<T>(value: T | undefined): T | null {
  return value ?? null;
}

/** Read `error.code` / `error.message` / `error.param` out of a body that may be anything at all. */
function readEnvelope(body: unknown): { code: string | null; message: string | null; param: string | null } {
  if (typeof body !== 'object' || body === null) return { code: null, message: null, param: null };
  const error = (body as { error?: unknown }).error;
  if (typeof error !== 'object' || error === null) return { code: null, message: null, param: null };
  const e = error as { code?: unknown; message?: unknown; param?: unknown };
  return {
    code: typeof e.code === 'string' ? e.code : null,
    // An explicit `""` must fall through to mapHttp's `HTTP <status>` fallback the same way a
    // missing or non-string message does — "Never empty" (spec § 9.5) means never, not "unless
    // the wire sent the empty string on purpose".
    message: typeof e.message === 'string' && e.message !== '' ? e.message : null,
    // A null param must not become the string "null" in metadata.
    param: typeof e.param === 'string' ? e.param : null,
  };
}

/** The presentation an override selects, else the transport table's answer. */
function resolve(reason: ErrorReason | undefined, fallback: Presentation): Presentation {
  return presentationOverride(orNull(reason)) ?? fallback;
}

export function mapError(input: ErrorInput): PaigasusError {
  switch (input.kind) {
    case 'grpc':
      return mapConnect(input.error);
    case 'http':
      return mapHttp(input.status, input.headers, input.body);
    case 'terminal-frame': {
      // The head was already committed, so the status cannot change — but it is not necessarily
      // 200. The gateway forwards the upstream's own success status, so a stream committed on
      // 201 must report 201. The HTTP table has no 2xx row at all; `upstream-error`'s OVERRIDE
      // entry is what makes this `degraded` rather than `generic` (spec § 9.4).
      const headers = new Headers();
      if (input.ids?.correlationId != null) headers.set(CORRELATION_HEADER, input.ids.correlationId);
      if (input.ids?.requestId != null) headers.set(REQUEST_ID_HEADER, input.ids.requestId);
      return mapHttp(input.status, headers, input.body);
    }
    case 'transport':
      return {
        presentation: presentationForTransportCause(input.cause),
        domain: null,
        reason: null,
        rawReason: null,
        rawDomain: null,
        message: input.message,
        correlationId: null,
        requestId: null,
        retryable: null,
        metadata: {},
        transport: { kind: 'transport', cause: input.cause },
      };
  }
}

function mapConnect(err: ConnectError): PaigasusError {
  const transport: TransportInfo = { kind: 'grpc', code: err.code, codeName: grpcCodeName(err.code) };
  const fallback = presentationForGrpcCode(err.code);
  // findDetails returns an EMPTY ARRAY with no signal when the error carries no detail — a
  // network failure, a proxy, or a connection reset. Branch BEFORE reading [0], or the SDK
  // throws while mapping an error (spec § 9.5 arm 1).
  const detail = err.findDetails(ErrorInfoSchema)[0];

  if (detail === undefined) {
    return {
      presentation: fallback,
      domain: null,
      reason: null,
      rawReason: null,
      rawDomain: null,
      // rawMessage, not message: ConnectError.message prefixes the status code (e.g.
      // "[not_found] first"), which would make the message carry branch-relevant information the
      // rest of mapError never reads — see AC 1's exact-message test.
      message: err.rawMessage,
      // err.metadata is a union of response headers and trailers, so it may still carry the id.
      correlationId: firstNonEmpty(err.metadata.get(CORRELATION_HEADER)),
      requestId: firstNonEmpty(err.metadata.get(REQUEST_ID_HEADER)),
      retryable: null,
      metadata: {},
      transport,
    };
  }

  const metadata: Record<string, string> = { ...detail.metadata };
  for (const key of LIFTED_KEYS) delete metadata[key];

  const reason = fromWireReason(detail.reason);
  return {
    presentation: resolve(reason, fallback),
    domain: orNull(fromWireDomain(detail.domain)),
    reason: orNull(reason),
    // Always what the wire SAID, whether or not it resolved.
    rawReason: detail.reason === '' ? null : detail.reason,
    rawDomain: detail.domain === '' ? null : detail.domain,
    // rawMessage, not message — see the no-detail branch above.
    message: err.rawMessage,
    // The metadata KEY is `correlation_id` (convert.rs:70); the HEADER is
    // `paigasus-correlation-id` (correlation.rs:31). Two spellings, both read, metadata first.
    // `??` does NOT fall through on an empty string, so an ErrorInfo carrying `correlation_id: ""`
    // would suppress the header fallback and yield a blank id. Ported from PR #231's own review.
    correlationId: firstNonEmpty(detail.metadata.correlation_id, err.metadata.get(CORRELATION_HEADER)),
    requestId: firstNonEmpty(detail.metadata.request_id, err.metadata.get(REQUEST_ID_HEADER)),
    retryable: parseRetryable(detail.metadata.retryable),
    metadata,
    transport,
  };
}

function mapHttp(status: number, headers: Headers, body: unknown): PaigasusError {
  const { code, message, param } = readEnvelope(body);
  const reason = code === null ? undefined : fromWireReason(code);
  const fallback = presentationForHttpStatus(status);

  return {
    presentation: resolve(reason, fallback),
    // Neither HTTP envelope carries a domain. Both are single-service bodies, so there is nothing
    // to read and nothing to preserve.
    domain: null,
    reason: orNull(reason),
    rawReason: code,
    rawDomain: null,
    // Never empty: a malformed or non-JSON body still yields something a user can report.
    message: message ?? `HTTP ${status}`,
    correlationId: firstNonEmpty(headers.get(CORRELATION_HEADER)),
    requestId: firstNonEmpty(headers.get(REQUEST_ID_HEADER)),
    retryable: parseRetryable(headers.get(RETRYABLE_HEADER)),
    metadata: param === null ? {} : { param },
    transport: { kind: 'http', status },
  };
}
