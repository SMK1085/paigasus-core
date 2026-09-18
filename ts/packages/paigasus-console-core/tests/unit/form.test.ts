// SPDX-License-Identifier: Apache-2.0
//
// The pure form helpers (SMA-636 D11). They moved here from iam-console's lib/form.ts with these
// tests. The name bound counts CODE POINTS, as IAM's NAME_MAX_CHARS does; zod's `.max()` counts
// UTF-16 units and would refuse a valid name of astral characters (SMA-630 spec § 4.2).
import { describe, expect, it } from 'vitest';
import { NAME_MAX_CODE_POINTS, formFields, invalidFormInput, nameField, prnField, toActionResult } from '../../src/form';

describe('the form helpers', () => {
  it('reads the named fields and nothing else', () => {
    const form = new FormData();
    form.set('slug', 'acme');
    form.set('extra', 'ignored');
    expect(formFields(form, ['slug', 'name'])).toEqual({ slug: 'acme', name: null });
  });

  it('builds a local invalid-input error that claims nothing about IAM', () => {
    const error = invalidFormInput();
    expect(error.presentation).toBe('invalid-input');
    expect(error.domain).toBeNull();
    expect(error.reason).toBeNull();
    expect(error.correlationId).toBeNull();
    expect(error.retryable).toBe(false);
    // A plain object, so it crosses the Flight boundary as an action result.
    expect(structuredClone(error)).toEqual(error);
  });

  it('maps an IamResult to an action result and drops the value', () => {
    expect(toActionResult({ ok: true, value: { anything: 1 } })).toEqual({ ok: true });
    const error = invalidFormInput();
    expect(toActionResult({ ok: false, error })).toEqual({ ok: false, error });
  });
});

describe('the shared field bounds', () => {
  const GRIN = '\u{1F600}';

  it('accepts a name of 256 code points and refuses 257', () => {
    expect(NAME_MAX_CODE_POINTS).toBe(256);
    expect(nameField.safeParse('a'.repeat(256)).success).toBe(true);
    expect(nameField.safeParse('a'.repeat(257)).success).toBe(false);
  });

  it('counts code points, not UTF-16 units: 256 astral characters pass', () => {
    const astral = GRIN.repeat(256);
    expect(astral.length).toBe(512);
    expect(nameField.safeParse(astral).success).toBe(true);
    expect(nameField.safeParse(`${astral}${GRIN}`).success).toBe(false);
  });

  it('trims a name before it counts, and refuses a name that is only whitespace', () => {
    expect(nameField.safeParse(`  ${'a'.repeat(256)}  `).data).toBe('a'.repeat(256));
    expect(nameField.safeParse('   ').success).toBe(false);
  });

  it('keeps the PRN bound at 512, trimmed', () => {
    expect(prnField.safeParse('p'.repeat(512)).success).toBe(true);
    expect(prnField.safeParse('p'.repeat(513)).success).toBe(false);
    expect(prnField.safeParse('  ').success).toBe(false);
  });
});
