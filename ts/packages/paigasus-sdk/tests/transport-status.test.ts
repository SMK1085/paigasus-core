// SPDX-License-Identifier: Apache-2.0
import { Code } from '@connectrpc/connect';
import { describe, expect, it } from 'vitest';

import { presentationForGrpcCode, presentationForHttpStatus, presentationForTransportCause } from '../src/errors/transport-status.js';
import type { Presentation } from '../src/errors/types.js';

const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'invalid-input', 'conflict', 'disabled', 'generic'];

describe('the gRPC status table', () => {
  // TOTALITY is the assertion that carries this suite. A transcription of the table into the
  // test only proves that someone transcribed it. MEASURED: Object.values(Code) yields 32
  // entries — 16 numeric and 16 string names — so the numeric filter is required, not tidiness.
  it('resolves every numeric Code member', () => {
    const numeric = Object.values(Code).filter((v): v is Code => typeof v === 'number');
    expect(numeric).toHaveLength(16);
    for (const code of numeric) {
      expect(PRESENTATIONS).toContain(presentationForGrpcCode(code));
    }
  });

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
    [Code.Unimplemented, 'disabled'],
    [Code.ResourceExhausted, 'degraded'],
  ] as const)('maps code %i to %s', (code, expected) => {
    expect(presentationForGrpcCode(code)).toBe(expected);
  });

  it('falls through to generic', () => {
    expect(presentationForGrpcCode(Code.Internal)).toBe('generic');
  });
});

describe('the HTTP status table', () => {
  it.each([
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
  ] as const)('maps %i to %s', (status, expected) => {
    expect(presentationForHttpStatus(status)).toBe(expected);
  });

  // 200 has no row on purpose. The terminal SSE frame arrives with status 200 because the head
  // was already committed, and its `degraded` presentation comes from the reason OVERRIDE table
  // in Task 3, never from here (spec § 9.4).
  it.each([200, 418, 500, 599])('falls through to generic for %i', (status) => {
    expect(presentationForHttpStatus(status)).toBe('generic');
  });
});

describe('the transport-cause table', () => {
  it.each([
    ['timeout', 'degraded'],
    ['network', 'degraded'],
    // A caller abort is not a service fault and must not render as one.
    ['aborted', 'generic'],
  ] as const)('maps %s to %s', (cause, expected) => {
    expect(presentationForTransportCause(cause)).toBe(expected);
  });
});
