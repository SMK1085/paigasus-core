// SPDX-License-Identifier: Apache-2.0
//
// The error copy table (spec § 6.5, § 9.2, D16).
import { describe, expect, it } from 'vitest';
import type { Presentation } from '@paigasus/sdk/errors/types';
import { PRESENTATION_COPY } from '../../app/_components/error-copy';

const PRESENTATIONS: readonly Presentation[] = ['relogin', 'forbidden', 'not-found', 'degraded', 'rate-limited', 'invalid-input', 'conflict', 'disabled', 'generic'];

describe('the error copy', () => {
  it('has a title and a body for every presentation', () => {
    // Iterates the literal list above, not Object.keys(PRESENTATION_COPY): a table replaced by
    // `{}` widened to `any` would still pass a key-driven assertion, since there would be nothing
    // to iterate. Walking PRESENTATIONS instead fails on a MISSING key, which is the case that
    // matters.
    expect(Object.keys(PRESENTATION_COPY).sort()).toEqual([...PRESENTATIONS].sort());
    for (const presentation of PRESENTATIONS) {
      expect(PRESENTATION_COPY[presentation].title.length).toBeGreaterThan(0);
      expect(PRESENTATION_COPY[presentation].body.length).toBeGreaterThan(0);
    }
  });
});
