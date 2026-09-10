// SPDX-License-Identifier: Apache-2.0
import '../server-guard.js';

import { ConnectError } from '@connectrpc/connect';
import { ErrorInfoSchema, fromWireDomain, fromWireReason } from '@paigasus/proto';
import type { ErrorDomain, ErrorReason } from '@paigasus/proto';

import { presentationFor } from './presentation.js';
import { presentationForGrpcCode, presentationForHttpStatus } from './transport-tables.js';
import type { PaigasusError, Presentation } from './types.js';

const CORRELATION_ID_HEADER = 'paigasus-correlation-id';
const REQUEST_ID_HEADER = 'paigasus-request-id';
const RETRYABLE_HEADER = 'paigasus-retryable';

/** The keys lifted out of the ErrorInfo map into typed fields, so no raw duplicate can disagree. */
const LIFTED_KEYS = new Set(['retryable', 'correlation_id', 'request_id']);

export interface FrameIds {
  readonly correlationId: string | null;
  readonly requestId: string | null;
}

/**
 * The four wire shapes this SDK can meet (spec § 9.5).
 *
 * One discriminated union rather than four exported functions: the switch below is exhaustive
 * against it, so a fifth shape is a compile error rather than a forgotten arm.
 */
export type MapErrorInput =
  | { readonly kind: 'grpc'; readonly error: ConnectError }
  | { readonly kind: 'iam-http'; readonly status: number; readonly headers: Headers; readonly body: unknown }
  | { readonly kind: 'gateway-http'; readonly status: number; readonly headers: Headers; readonly body: unknown }
  | { readonly kind: 'terminal-frame'; readonly status: number; readonly body: unknown; readonly ids: FrameIds };

/**
 * The wire's retryable is exactly "true" | "false" | "unknown" (correlation.rs:59-65). ANYTHING
 * else — including an absent header — is `null`, never `false`: collapsing it would assert a
 * non-retryability the service declined to assert (ADR-0019 decision 7).
 */
function parseRetryable(raw: string | null | undefined): boolean | null {
  if (raw === 'true') return true;
  if (raw === 'false') return false;
  return null;
}

/** Resolve a wire code, keeping the raw value whether or not it resolves. */
function resolveReason(raw: string | null): { reason: ErrorReason | null; rawReason: string | null } {
  // An empty code is not a wire value — it is an absent one, so rawReason is null, never ''.
  if (raw === null || raw === '') return { reason: null, rawReason: null };
  return { reason: fromWireReason(raw) ?? null, rawReason: raw };
}

function resolveDomain(raw: string | null): { domain: ErrorDomain | null; rawDomain: string | null } {
  if (raw === null || raw === '') return { domain: null, rawDomain: null };
  return { domain: fromWireDomain(raw) ?? null, rawDomain: raw };
}

/** An override wins; `'from-transport'` defers to the status-derived value. */
function applyOverride(reason: ErrorReason | null, fromTransport: Presentation): Presentation {
  if (reason === null) return fromTransport;
  const entry = presentationFor(reason);
  return entry === 'from-transport' ? fromTransport : entry;
}

/** Read `{error:{...}}` defensively — the body may be a string, null, or an unrelated object. */
function errorObject(body: unknown): Record<string, unknown> | null {
  if (typeof body !== 'object' || body === null) return null;
  const candidate = (body as { error?: unknown }).error;
  if (typeof candidate !== 'object' || candidate === null) return null;
  return candidate as Record<string, unknown>;
}

function stringField(source: Record<string, unknown> | null, key: string): string | null {
  const value = source?.[key];
  return typeof value === 'string' ? value : null;
}

/**
 * Map any wire failure onto one `PaigasusError`.
 *
 * Branches on `(domain, reason)` and the transport status ONLY. No arm reads `message` — AC 2 is
 * satisfied structurally, not by convention.
 */
export function mapError(input: MapErrorInput): PaigasusError {
  switch (input.kind) {
    case 'grpc':
      return mapConnect(input.error);
    case 'iam-http':
      return mapHttpEnvelope(input.status, input.headers, input.body, 'iam');
    case 'gateway-http':
      return mapHttpEnvelope(input.status, input.headers, input.body, 'gateway');
    case 'terminal-frame':
      return mapTerminalFrame(input.status, input.body, input.ids);
  }
}

function mapConnect(error: ConnectError): PaigasusError {
  const fromTransport = presentationForGrpcCode(error.code);
  // findDetails returns an ARRAY — `[0]` is undefined whenever no detail rode along, which is a
  // reachable path (a network failure, a proxy, a connection reset). Without this branch the SDK
  // throws while mapping an error, turning a recoverable failure into an unhandled exception.
  const info = error.findDetails(ErrorInfoSchema)[0];
  // Even with no detail, `metadata` is a union of response headers and trailers and may carry it.
  const headerCorrelation = error.metadata.get(CORRELATION_ID_HEADER);

  if (info === undefined) {
    return {
      presentation: fromTransport,
      domain: null,
      reason: null,
      rawReason: null,
      rawDomain: null,
      // rawMessage, not message: ConnectError prefixes the latter with "[code] ".
      message: error.rawMessage,
      correlationId: headerCorrelation ?? null,
      requestId: error.metadata.get(REQUEST_ID_HEADER) ?? null,
      retryable: null,
      metadata: {},
      transport: { kind: 'grpc', code: error.code },
    };
  }

  const { reason, rawReason } = resolveReason(info.reason === '' ? null : info.reason);
  const { domain, rawDomain } = resolveDomain(info.domain === '' ? null : info.domain);
  const metadata: Record<string, string> = {};
  for (const [key, value] of Object.entries(info.metadata)) {
    if (!LIFTED_KEYS.has(key)) metadata[key] = value;
  }

  return {
    presentation: applyOverride(reason, fromTransport),
    domain,
    reason,
    rawReason,
    rawDomain,
    message: error.rawMessage,
    // The ids are OMITTED from ErrorInfo.metadata outside a request scope (convert.rs:69-72), so
    // the header fallback is not decoration.
    correlationId: info.metadata['correlation_id'] ?? headerCorrelation ?? null,
    requestId: info.metadata['request_id'] ?? error.metadata.get(REQUEST_ID_HEADER) ?? null,
    retryable: parseRetryable(info.metadata['retryable']),
    metadata,
    transport: { kind: 'grpc', code: error.code },
  };
}

/**
 * Arms 2 and 3. One body, because IAM's `{error:{code,message}}` is the gateway's envelope minus
 * two fields, and both carry the same three response headers.
 *
 * The gateway arm must tolerate far more than the IAM arm: a non-2xx chat body is usually the
 * UPSTREAM's, forwarded verbatim under a forced application/json (chat.rs:19-20). So the body may
 * be a string, may carry OpenAI's underscored vocabulary, or may have no `error` object at all.
 * None of those throws; each degrades to the status table with the correlation id preserved.
 */
function mapHttpEnvelope(status: number, headers: Headers, body: unknown, source: 'iam' | 'gateway'): PaigasusError {
  const fromTransport = presentationForHttpStatus(status);
  const errorObj = errorObject(body);
  const { reason, rawReason } = resolveReason(stringField(errorObj, 'code'));

  const metadata: Record<string, string> = {};
  if (source === 'gateway') {
    // `param` is Option<String>: a JSON null is OMITTED, never stringified to "null".
    const param = stringField(errorObj, 'param');
    if (param !== null) metadata['param'] = param;
  }

  return {
    presentation: applyOverride(reason, fromTransport),
    // Neither HTTP envelope has a domain field, structurally (spec § 9.1).
    domain: null,
    reason,
    rawReason,
    rawDomain: null,
    message: stringField(errorObj, 'message') ?? `HTTP ${String(status)}`,
    correlationId: headers.get(CORRELATION_ID_HEADER),
    requestId: headers.get(REQUEST_ID_HEADER),
    retryable: parseRetryable(headers.get(RETRYABLE_HEADER)),
    metadata,
    transport: { kind: 'http', status },
  };
}

function mapTerminalFrame(status: number, body: unknown, ids: FrameIds): PaigasusError {
  const errorObj = errorObject(body);
  const { reason, rawReason } = resolveReason(stringField(errorObj, 'code'));

  return {
    presentation: applyOverride(reason, presentationForHttpStatus(status)),
    domain: null,
    reason,
    rawReason,
    rawDomain: null,
    message: stringField(errorObj, 'message') ?? 'upstream stream error',
    correlationId: ids.correlationId,
    requestId: ids.requestId,
    // The frame deliberately carries no retryable signal: the 200 head is already committed, so no
    // header can change (chat.rs:56-62).
    retryable: null,
    metadata: {},
    // The COMMITTED 2xx status — chat.rs:128 branches on is_success(), so 200 is usual, not the
    // only reachable value.
    transport: { kind: 'http', status },
  };
}
