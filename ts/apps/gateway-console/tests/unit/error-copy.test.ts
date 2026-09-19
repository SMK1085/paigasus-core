// SPDX-License-Identifier: Apache-2.0
//
// The error copy tables (spec § 6.5, § 9.2, D16; SMA-636 § 5.1). The form table covers the
// reasons the service-account forms can get.
import { describe, expect, it } from 'vitest';
import { ErrorReason, type PaigasusError, type Presentation } from '@paigasus/sdk/errors/types';
import { FORM_REASON_COPY, PRESENTATION_COPY, formMessage } from '../../app/_components/error-copy';

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
    // Iterates the literal list above, not Object.keys(PRESENTATION_COPY): a table replaced by
    // `{}` widened to `any` would still pass a key-driven assertion.
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

  it('says the service-account name conflict in its own words (SMA-636 § 5.2)', () => {
    expect(formMessage(errorWith('conflict', ErrorReason.SERVICE_ACCOUNT_NAME_CONFLICT))).toBe('A service account with this name already exists here.');
  });

  it('falls back to the presentation copy for a reason the table does not have, and never shows IAM’s message', () => {
    const message = formMessage(errorWith('degraded', null));
    expect(message).toBe(PRESENTATION_COPY.degraded.body);
    expect(message).not.toContain('IAM text');
  });
});
