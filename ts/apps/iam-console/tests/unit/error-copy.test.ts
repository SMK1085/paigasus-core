// SPDX-License-Identifier: Apache-2.0
//
// The error copy tables (spec § 6.5, § 9.2).
import { describe, expect, it } from 'vitest';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY, formMessage, PRESENTATION_COPY } from '../../app/_components/error-copy';

const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'rate-limited', 'invalid-input', 'conflict', 'disabled', 'generic'];

function errorWith(presentation: Presentation, reason: ErrorReason | null): PaigasusError {
  return {
    presentation,
    domain: null,
    reason,
    rawReason: null,
    rawDomain: null,
    message: 'IAM text that must never show',
    correlationId: 'cid-1',
    requestId: null,
    retryable: null,
    metadata: {},
    transport: { kind: 'transport', cause: 'network' },
  };
}

describe('the error copy', () => {
  it('has a title and a body for every presentation', () => {
    expect(Object.keys(PRESENTATION_COPY).sort()).toEqual([...PRESENTATIONS].sort());
    for (const presentation of PRESENTATIONS) {
      expect(PRESENTATION_COPY[presentation].title.length).toBeGreaterThan(0);
      expect(PRESENTATION_COPY[presentation].body.length).toBeGreaterThan(0);
    }
  });

  it('keys the form table only by real, non-sentinel ErrorReason values', () => {
    const keys = Object.keys(FORM_REASON_COPY).map(Number);
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) {
      expect(ErrorReason[key]).toBeDefined();
      expect(key).not.toBe(ErrorReason.UNSPECIFIED);
    }
  });

  it('prefers the reason copy, and falls back to the presentation copy for an unknown reason', () => {
    expect(formMessage(errorWith('conflict', ErrorReason.SLUG_CONFLICT))).toBe(FORM_REASON_COPY[ErrorReason.SLUG_CONFLICT]);
    expect(formMessage(errorWith('conflict', null))).toBe(PRESENTATION_COPY.conflict.body);
    expect(formMessage(errorWith('generic', ErrorReason.INTERNAL))).toBe(PRESENTATION_COPY.generic.body);
  });

  it('never repeats the message IAM sent', () => {
    for (const presentation of PRESENTATIONS) {
      expect(formMessage(errorWith(presentation, null))).not.toContain('IAM text');
    }
  });

  it('gives a form the spec § 6.1 "not enabled" copy for a disabled capability, and other copy for an inactive principal', () => {
    expect(formMessage(errorWith('disabled', ErrorReason.CAPABILITY_DISABLED))).toBe('This feature is not enabled on this IAM.');
    const inactive = formMessage(errorWith('disabled', ErrorReason.PRINCIPAL_INACTIVE));
    expect(inactive).toBe('Your account is not active in IAM.');
    expect(inactive).not.toContain('not enabled');
  });
});
