// SPDX-License-Identifier: Apache-2.0
import { create, toBinary } from '@bufbuild/protobuf';
import { Code, ConnectError } from '@connectrpc/connect';
import { ErrorDomain, ErrorInfoSchema, ErrorReason } from '@paigasus/proto';
import { describe, expect, it } from 'vitest';

import { mapError } from '../src/errors/map-error.js';

/**
 * Build a ConnectError carrying an ErrorInfo detail in the INCOMING WIRE SHAPE.
 *
 * This matters. A detail supplied as `{ desc, value }` takes findDetails's OUTGOING branch — a
 * plain create() — and proves nothing about decoding a `grpc-status-details-bin` trailer of the
 * kind tonic_types::ErrorDetails::with_error_info produces (convert.rs:79). The `{ type, value }`
 * form takes the fromBinary branch, which is the path under test (spec § 10).
 *
 * protobuf-es v2's `toBinary` expects a `Message`, so the ErrorInfo is built with `create()`
 * rather than a raw object literal carrying `$typeName` (task-4 ruling).
 */
function connectErrorWithDetail(message: string, code: Code, metadata: Record<string, string>, reason = 'slug-conflict', domain = 'iam.paigasus.io'): ConnectError {
  const info = create(ErrorInfoSchema, { reason, domain, metadata });
  const bytes = toBinary(ErrorInfoSchema, info);
  const err = new ConnectError(message, code);
  // ConnectError's constructor types `outgoingDetails` as `OutgoingDetail[]` ({ desc, value }), so
  // an incoming-wire-shape detail ({ type, value: Uint8Array }) cannot be passed there without a
  // cast. `details` itself is typed `(OutgoingDetail | IncomingDetail)[]` and is assigned
  // identically inside the constructor (`this.details = outgoingDetails ?? []`), so setting it
  // after construction is the same value with no cast needed.
  err.details = [{ type: 'google.rpc.ErrorInfo', value: bytes }];
  return err;
}

describe('arm 1 — a ConnectError carrying an ErrorInfo detail', () => {
  it('round-trips the pair, lifts the three keys, and keeps the rest', () => {
    const err = connectErrorWithDetail('conflict', Code.AlreadyExists, {
      retryable: 'false',
      correlation_id: 'corr-1',
      request_id: 'req-1',
      capability: 'chat_stream',
      field: 'slug',
    });

    const result = mapError({ kind: 'grpc', error: err });

    expect(result.reason).toBe(ErrorReason.SLUG_CONFLICT);
    expect(result.domain).toBe(ErrorDomain.IAM);
    expect(result.rawReason).toBe('slug-conflict');
    expect(result.rawDomain).toBe('iam.paigasus.io');
    expect(result.correlationId).toBe('corr-1');
    expect(result.requestId).toBe('req-1');
    expect(result.retryable).toBe(false);
    expect(result.presentation).toBe('conflict');
    // The three lifted keys are REMOVED, so a caller cannot branch on a raw duplicate that
    // disagrees with the parsed field.
    expect(result.metadata).toEqual({ capability: 'chat_stream', field: 'slug' });
    expect(result.transport).toEqual({ kind: 'grpc', code: Code.AlreadyExists, codeName: 'AlreadyExists' });
  });

  it('treats an unknown retryable as null rather than false', () => {
    const err = connectErrorWithDetail('x', Code.Internal, { retryable: 'unknown' });
    expect(mapError({ kind: 'grpc', error: err }).retryable).toBeNull();
  });

  it('keeps rawReason and correlationId when the reason is unknown', () => {
    const err = connectErrorWithDetail('x', Code.NotFound, { correlation_id: 'corr-2' }, 'no-such-code', 'iam.paigasus.io');
    const result = mapError({ kind: 'grpc', error: err });
    expect(result.reason).toBeNull();
    expect(result.rawReason).toBe('no-such-code');
    expect(result.correlationId).toBe('corr-2');
    expect(result.presentation).toBe('not-found');
  });

  it('keeps rawDomain when the domain is unknown', () => {
    const err = connectErrorWithDetail('x', Code.Internal, {}, 'slug-conflict', 'billing');
    const result = mapError({ kind: 'grpc', error: err });
    expect(result.domain).toBeNull();
    expect(result.rawDomain).toBe('billing');
  });
});

describe('arm 1b — a ConnectError with NO ErrorInfo detail', () => {
  // Reachable, not hypothetical: a network failure, a proxy or a connection reset raises a
  // ConnectError carrying a status and no detail. Without this branch the SDK THROWS while
  // mapping an error, turning a recoverable upstream failure into an unhandled BFF exception.
  it('falls back to the status table and throws nothing', () => {
    const result = mapError({ kind: 'grpc', error: new ConnectError('connection reset', Code.Unavailable) });
    expect(result.reason).toBeNull();
    expect(result.domain).toBeNull();
    expect(result.rawReason).toBeNull();
    expect(result.rawDomain).toBeNull();
    expect(result.retryable).toBeNull();
    expect(result.metadata).toEqual({});
    expect(result.presentation).toBe('degraded');
    expect(result.message).toContain('connection reset');
  });

  it('still reads a correlation id from the header union', () => {
    const headers = new Headers({ 'paigasus-correlation-id': 'corr-3' });
    const result = mapError({ kind: 'grpc', error: new ConnectError('x', Code.Internal, headers) });
    expect(result.correlationId).toBe('corr-3');
  });
});

describe("arm 2 — IAM's HTTP envelope", () => {
  it('reads the correlation id and retryable from the HEADERS, not the body', () => {
    const result = mapError({
      kind: 'http',
      status: 422,
      headers: new Headers({ 'paigasus-correlation-id': 'corr-4', 'paigasus-request-id': 'req-4', 'paigasus-retryable': 'true' }),
      body: { error: { code: 'invalid-request-schema', message: 'bad shape' } },
    });

    expect(result.correlationId).toBe('corr-4');
    expect(result.requestId).toBe('req-4');
    expect(result.retryable).toBe(true);
    expect(result.reason).toBe(ErrorReason.INVALID_REQUEST_SCHEMA);
    expect(result.presentation).toBe('invalid-input');
    // IAM's HTTP body has no metadata map at all — `field` is gRPC-only (convert.rs:129-130).
    expect(result.metadata).toEqual({});
    expect(result.transport).toEqual({ kind: 'http', status: 422 });
  });
});

describe("arm 3 — the gateway's OpenAI envelope", () => {
  it('carries param into metadata and pins the 400/422 divergence', () => {
    const result = mapError({
      kind: 'http',
      status: 400,
      headers: new Headers({ 'paigasus-retryable': 'false' }),
      body: { error: { message: 'bad shape', type: 'invalid_request_error', param: 'model', code: 'invalid-request-schema' } },
    });

    expect(result.reason).toBe(ErrorReason.INVALID_REQUEST_SCHEMA);
    // The SAME code as arm 2's 422 resolves to the SAME presentation. That agreement is what the
    // override entry pins (error.proto:239-247).
    expect(result.presentation).toBe('invalid-input');
    expect(result.metadata).toEqual({ param: 'model' });
  });

  // The gateway forwards a non-2xx upstream response VERBATIM (chat.rs:113-119), so this body is
  // OpenAI's own vocabulary, not the registry's.
  it('degrades an upstream passthrough with an underscore code', () => {
    const result = mapError({
      kind: 'http',
      status: 429,
      headers: new Headers(),
      body: { error: { message: 'You exceeded your current quota', type: 'insufficient_quota', param: null, code: 'insufficient_quota' } },
    });

    expect(result.reason).toBeNull();
    expect(result.rawReason).toBe('insufficient_quota');
    expect(result.presentation).toBe('degraded');
    expect(result.retryable).toBeNull();
    // A null param must not enter metadata as the string "null".
    expect(result.metadata).toEqual({});
  });

  it('handles a null code', () => {
    const result = mapError({ kind: 'http', status: 500, headers: new Headers(), body: { error: { message: 'boom', code: null, param: null } } });
    expect(result.reason).toBeNull();
    expect(result.rawReason).toBeNull();
    expect(result.presentation).toBe('generic');
  });
});

describe('mapError is total over a malformed body', () => {
  // mapError runs in the BFF on the FAILURE path. A throw here turns a recoverable upstream
  // failure into an unhandled exception — the same failure mode arm 1b exists to prevent.
  it.each([
    ['an HTML body', '<html><body>502 Bad Gateway</body></html>'],
    ['an empty string', ''],
    ['undefined', undefined],
    ['a JSON body with no error key', { detail: 'nope' }],
    ['a JSON null', null],
    ['an explicit empty message', { error: { message: '', code: null, param: null } }],
  ])('does not throw on %s', (_label, body) => {
    const result = mapError({ kind: 'http', status: 502, headers: new Headers(), body });
    expect(result.reason).toBeNull();
    expect(result.rawReason).toBeNull();
    expect(result.presentation).toBe('degraded');
    expect(result.message).not.toBe('');
  });
});

describe('arm 5 — a transport failure with no response at all', () => {
  it.each([
    ['timeout', 'degraded'],
    ['network', 'degraded'],
    ['aborted', 'generic'],
  ] as const)('maps %s to %s', (cause, expected) => {
    const result = mapError({ kind: 'transport', cause, message: 'no response' });
    expect(result.presentation).toBe(expected);
    expect(result.transport).toEqual({ kind: 'transport', cause });
    expect(result.reason).toBeNull();
    expect(result.retryable).toBeNull();
  });
});

describe('AC 1 — the message is never an input to a branch', () => {
  it('maps one wire error with three messages to three identical objects modulo message', () => {
    const results = ['first', 'second', 'third'].map((m) => mapError({ kind: 'grpc', error: connectErrorWithDetail(m, Code.NotFound, { retryable: 'false' }) }));

    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- rest-sibling destructuring
    const stripped = results.map(({ message, ...rest }) => rest);
    expect(stripped[0]).toEqual(stripped[1]);
    expect(stripped[1]).toEqual(stripped[2]);
    expect(results.map((r) => r.message)).toEqual(['first', 'second', 'third']);
  });
});

describe('AC 3 — nothing raw reaches the browser', () => {
  it('returns a PLAIN object, never an Error subclass', () => {
    const result = mapError({ kind: 'grpc', error: new ConnectError('x', Code.Internal) });
    expect(Object.getPrototypeOf(result)).toBe(Object.prototype);
    expect(result).not.toBeInstanceOf(Error);
  });

  it('carries no ConnectError and no Headers', () => {
    const headers = new Headers({ 'paigasus-correlation-id': 'corr-5' });
    const result = mapError({ kind: 'grpc', error: new ConnectError('x', Code.Internal, headers) });
    for (const value of Object.values(result)) {
      expect(value).not.toBeInstanceOf(Headers);
      expect(value).not.toBeInstanceOf(ConnectError);
    }
    // The whole object must survive the RSC boundary, which is what AC 3 turns on.
    expect(() => structuredClone(result)).not.toThrow();
  });

  it.each([
    [Code.Unauthenticated, 'relogin'],
    [Code.PermissionDenied, 'forbidden'],
    [Code.NotFound, 'not-found'],
    [Code.Unavailable, 'degraded'],
  ] as const)('maps %i to %s', (code, expected) => {
    expect(mapError({ kind: 'grpc', error: new ConnectError('x', code) }).presentation).toBe(expected);
  });

  it.each([
    [401, 'relogin'],
    [403, 'forbidden'],
    [404, 'not-found'],
    [503, 'degraded'],
  ] as const)('maps HTTP %i to %s', (status, expected) => {
    expect(mapError({ kind: 'http', status, headers: new Headers(), body: {} }).presentation).toBe(expected);
  });
});
