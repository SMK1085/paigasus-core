// SPDX-License-Identifier: Apache-2.0
import { Code } from '@connectrpc/connect';
import { describe, expect, it } from 'vitest';

import { presentationForGrpcCode, presentationForHttpStatus } from '../src/errors/transport-tables.js';

describe('the gRPC status table (spec § 9.2)', () => {
  it.each([
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
  ] as const)('maps %s to %s', (code, expected) => {
    expect(presentationForGrpcCode(code)).toBe(expected);
  });

  // The row that makes § 9.4's override table load-bearing. Revision 1 mapped Unimplemented to
  // `disabled`, which made the CAPABILITY_DISABLED entry a no-op — the table's own justification,
  // defeated by the table above it. If this assertion is ever "fixed" to `disabled`, read § 9.2
  // before changing it.
  it('maps Unimplemented to generic, NOT disabled', () => {
    expect(presentationForGrpcCode(Code.Unimplemented)).toBe('generic');
  });

  it('falls through to generic', () => {
    expect(presentationForGrpcCode(Code.Internal)).toBe('generic');
    expect(presentationForGrpcCode(Code.Canceled)).toBe('generic');
    expect(presentationForGrpcCode(Code.Unknown)).toBe('generic');
  });
});

describe('the HTTP status table (spec § 9.2)', () => {
  it.each([
    [401, 'relogin'],
    [403, 'forbidden'],
    [404, 'not-found'],
    [408, 'degraded'],
    [409, 'conflict'],
    [413, 'invalid-input'],
    [415, 'invalid-input'],
    [422, 'invalid-input'],
    [400, 'invalid-input'],
    [502, 'degraded'],
    [503, 'degraded'],
    [504, 'degraded'],
  ] as const)('maps %i to %s', (status, expected) => {
    expect(presentationForHttpStatus(status)).toBe(expected);
  });

  // 429 is NOT degraded. Every 429 the SDK can currently see is OpenAI's quota arriving through
  // the gateway's passthrough (spec M13: nothing in this repo emits 429 itself).
  it('maps 429 to rate-limited, NOT degraded', () => {
    expect(presentationForHttpStatus(429)).toBe('rate-limited');
  });

  it('falls through to generic', () => {
    expect(presentationForHttpStatus(500)).toBe('generic');
    expect(presentationForHttpStatus(418)).toBe('generic');
    expect(presentationForHttpStatus(200)).toBe('generic');
  });
});
