// SPDX-License-Identifier: Apache-2.0
import { Code, ConnectError } from '@connectrpc/connect';
import { create } from '@bufbuild/protobuf';
import { ErrorDomain, ErrorInfoSchema, ErrorReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { mapError } from '../src/errors/map-error.js';

/** A ConnectError carrying a real ErrorInfo detail, built the way the wire builds one. */
function grpcError(overrides: { code?: Code; reason?: string; domain?: string; metadata?: Record<string, string>; message?: string } = {}): ConnectError {
  const info = create(ErrorInfoSchema, {
    reason: overrides.reason ?? 'slug-conflict',
    domain: overrides.domain ?? 'iam.paigasus.io',
    metadata: overrides.metadata ?? { retryable: 'false', correlation_id: 'corr-1', request_id: 'req-1' },
  });
  return new ConnectError(overrides.message ?? 'the slug is taken', overrides.code ?? Code.AlreadyExists, undefined, [{ desc: ErrorInfoSchema, value: info }]);
}

function headers(init: Record<string, string> = {}): Headers {
  return new Headers({ 'paigasus-correlation-id': 'corr-1', 'paigasus-request-id': 'req-1', ...init });
}

/** Recursively checks whether any own value in `value` is a `Headers` or a `ConnectError`. */
function containsLeak(value: unknown): boolean {
  if (value instanceof Headers || value instanceof ConnectError) return true;
  if (Array.isArray(value)) return value.some(containsLeak);
  if (value !== null && typeof value === 'object') return Object.values(value).some(containsLeak);
  return false;
}

describe('arm 1 — a ConnectError with an ErrorInfo detail', () => {
  it('resolves the pair, lifts the three keys, and keeps the rest of metadata', () => {
    const mapped = mapError({
      kind: 'grpc',
      error: grpcError({ metadata: { retryable: 'true', correlation_id: 'corr-1', request_id: 'req-1', field: 'slug' } }),
    });

    expect(mapped.reason).toBe(ErrorReason.SLUG_CONFLICT);
    expect(mapped.domain).toBe(ErrorDomain.IAM);
    expect(mapped.rawReason).toBe('slug-conflict');
    expect(mapped.rawDomain).toBe('iam.paigasus.io');
    expect(mapped.retryable).toBe(true);
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.requestId).toBe('req-1');
    expect(mapped.presentation).toBe('conflict');
    // The three lifted keys are gone; everything else survives.
    expect(mapped.metadata).toEqual({ field: 'slug' });
  });

  it('uses rawMessage, so no [code] prefix leaks into a UI string', () => {
    // ConnectError prefixes `message` with the status: "[already_exists] the slug is taken".
    const mapped = mapError({ kind: 'grpc', error: grpcError({ message: 'the slug is taken' }) });
    expect(mapped.message).toBe('the slug is taken');
    expect(mapped.message).not.toContain('[');
  });

  it('carries the transport code and nothing that could leak Headers', () => {
    const mapped = mapError({ kind: 'grpc', error: grpcError() });
    expect(mapped.transport).toEqual({ kind: 'grpc', code: Code.AlreadyExists });
    // AC 4: no ConnectError, no Headers anywhere in the object. Structural, not string-based:
    // JSON.stringify(new Headers()) is '{}', so a text search cannot see a leaked Headers value.
    expect(containsLeak(mapped)).toBe(false);
  });

  it('reads a tri-state retryable, and maps an unexpected value to null', () => {
    const unknown = mapError({ kind: 'grpc', error: grpcError({ metadata: { retryable: 'unknown' } }) });
    expect(unknown.retryable).toBeNull();
    const nonsense = mapError({ kind: 'grpc', error: grpcError({ metadata: { retryable: 'perhaps' } }) });
    expect(nonsense.retryable).toBeNull();
  });
});

describe('arm 1 — a ConnectError with NO detail', () => {
  // Reachable: a network failure, a proxy, or a connection reset carries a status and no
  // ErrorInfo. Without this branch the SDK throws WHILE MAPPING an error.
  it('falls back to the status table with a message-only object', () => {
    const mapped = mapError({ kind: 'grpc', error: new ConnectError('connection reset', Code.Unavailable) });

    expect(mapped.reason).toBeNull();
    expect(mapped.domain).toBeNull();
    expect(mapped.rawReason).toBeNull();
    expect(mapped.rawDomain).toBeNull();
    expect(mapped.retryable).toBeNull();
    expect(mapped.requestId).toBeNull();
    expect(mapped.metadata).toEqual({});
    expect(mapped.presentation).toBe('degraded');
    expect(mapped.message).toBe('connection reset');
  });

  it('still reads a correlation id from the error metadata', () => {
    // ConnectError.metadata is "a union of response headers and trailers", so it can carry the id
    // even when no ErrorInfo detail rode along.
    const err = new ConnectError('connection reset', Code.Unavailable, { 'paigasus-correlation-id': 'corr-9' });
    expect(mapError({ kind: 'grpc', error: err }).correlationId).toBe('corr-9');
  });
});

describe('arm 2 — an IAM HTTP response', () => {
  it('reads the ids and retryable from HEADERS, not the body', () => {
    const mapped = mapError({
      kind: 'iam-http',
      status: 409,
      headers: headers({ 'paigasus-retryable': 'false' }),
      body: { error: { code: 'slug-conflict', message: 'the slug is taken' } },
    });

    expect(mapped.reason).toBe(ErrorReason.SLUG_CONFLICT);
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.requestId).toBe('req-1');
    expect(mapped.retryable).toBe(false);
    expect(mapped.presentation).toBe('conflict');
    expect(mapped.transport).toEqual({ kind: 'http', status: 409 });
    // IAM's envelope has no domain field at all.
    expect(mapped.domain).toBeNull();
    expect(mapped.rawDomain).toBeNull();
  });

  it('treats a missing retryable header as unknown, not false', () => {
    const mapped = mapError({ kind: 'iam-http', status: 409, headers: headers(), body: { error: { code: 'slug-conflict', message: 'x' } } });
    expect(mapped.retryable).toBeNull();
  });
});

describe('arm 3 — the gateway, its own envelope', () => {
  it('maps a registry code and omits a null param', () => {
    const mapped = mapError({
      kind: 'gateway-http',
      status: 400,
      headers: headers({ 'paigasus-retryable': 'false' }),
      body: { error: { message: 'streaming is disabled', type: 'invalid_request_error', param: null, code: 'streaming-disabled' } },
    });

    expect(mapped.reason).toBe(ErrorReason.STREAMING_DISABLED);
    expect(mapped.presentation).toBe('invalid-input');
    // A JSON null must be OMITTED, never stringified to "null".
    expect(mapped.metadata).toEqual({});
  });

  it('carries a present param into metadata', () => {
    const mapped = mapError({
      kind: 'gateway-http',
      status: 400,
      headers: headers(),
      body: { error: { message: 'streaming is disabled', type: 'invalid_request_error', param: 'stream', code: 'streaming-disabled' } },
    });
    expect(mapped.metadata).toEqual({ param: 'stream' });
  });
});

describe('arm 3 — the gateway, an upstream OpenAI passthrough', () => {
  // THE ORDINARY PATH, not an edge case. The gateway forwards a non-2xx upstream body verbatim
  // (chat.rs:19-20), so `code` is OpenAI's vocabulary. This is AC 2's degradation requirement
  // doing its job on the SDK's most-used error surface.
  it('degrades an OpenAI code to reason null while keeping rawReason', () => {
    const mapped = mapError({
      kind: 'gateway-http',
      status: 429,
      headers: headers(),
      body: { error: { message: 'You exceeded your current quota', type: 'insufficient_quota', param: null, code: 'insufficient_quota' } },
    });

    expect(mapped.reason).toBeNull();
    expect(mapped.rawReason).toBe('insufficient_quota');
    expect(mapped.presentation).toBe('rate-limited');
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.message).toBe('You exceeded your current quota');
  });

  // The content-type is FORCED to application/json (chat.rs:118, :141) regardless of what the
  // upstream sent, so an HTML error page arrives labelled JSON. Mapping must not throw.
  it('tolerates a body that is not an object at all', () => {
    const mapped = mapError({ kind: 'gateway-http', status: 502, headers: headers(), body: '<html>502 Bad Gateway</html>' });
    expect(mapped.reason).toBeNull();
    expect(mapped.rawReason).toBeNull();
    expect(mapped.presentation).toBe('degraded');
    expect(mapped.correlationId).toBe('corr-1');
    expect(mapped.message).not.toBe('');
  });

  it('tolerates a JSON body with no error object', () => {
    const mapped = mapError({ kind: 'gateway-http', status: 500, headers: headers(), body: { detail: 'something else entirely' } });
    expect(mapped.reason).toBeNull();
    expect(mapped.presentation).toBe('generic');
    expect(mapped.message).not.toBe('');
  });
});

describe('AC 2 — the branch never reads message text', () => {
  it('maps one wire error with three messages to three identical objects modulo message', () => {
    const mapped = ['the slug is taken', '', 'ERROR: Slug conflict (retry?)'].map((message) => mapError({ kind: 'grpc', error: grpcError({ message }) }));

    const withoutMessage = mapped.map((m) => ({ ...m, message: undefined }));
    expect(withoutMessage[1]).toEqual(withoutMessage[0]);
    expect(withoutMessage[2]).toEqual(withoutMessage[0]);
    // ...and the messages really did differ, so the assertion above is not vacuous.
    expect(new Set(mapped.map((m) => m.message)).size).toBe(3);
  });
});

describe('AC 4 — the four codes name four states', () => {
  it.each([
    [Code.Unauthenticated, 'relogin'],
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unavailable, 'degraded'],
  ] as const)('%s yields %s', (code, expected) => {
    expect(mapError({ kind: 'grpc', error: new ConnectError('x', code) }).presentation).toBe(expected);
  });
});

describe('AC 2 — an unknown domain degrades without losing the wire value', () => {
  it('keeps rawDomain when the domain does not resolve', () => {
    const mapped = mapError({ kind: 'grpc', error: grpcError({ domain: 'billing.paigasus.io' }) });
    expect(mapped.domain).toBeNull();
    expect(mapped.rawDomain).toBe('billing.paigasus.io');
  });
});
