// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { formFields, invalidFormInput, toActionResult } from '../../lib/form';

describe('lib/form', () => {
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
    // A plain object, so it crosses the Flight boundary as an action result (spec § 5.3).
    expect(structuredClone(error)).toEqual(error);
  });

  it('maps an IamResult to an action result and drops the value', () => {
    expect(toActionResult({ ok: true, value: { anything: 1 } })).toEqual({ ok: true });
    const error = invalidFormInput();
    expect(toActionResult({ ok: false, error })).toEqual({ ok: false, error });
  });
});
