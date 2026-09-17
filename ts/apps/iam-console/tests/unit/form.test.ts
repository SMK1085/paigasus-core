// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from 'vitest';
import { currentField, formFields, invalidFormInput, nameField, NAME_MAX_CODE_POINTS, prnField, renameChange, slugField, toActionResult } from '../../lib/form';

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

// SMA-630 spec § 4.2. The name bound counts CODE POINTS, as IAM's NAME_MAX_CHARS does. zod's
// `.max()` counts UTF-16 units, so the old 200-unit bound refused a valid name that IAM stored.
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

  it('keeps the slug bound at 200 and the PRN bound at 512, both trimmed', () => {
    expect(slugField.safeParse('s'.repeat(200)).success).toBe(true);
    expect(slugField.safeParse('s'.repeat(201)).success).toBe(false);
    expect(slugField.safeParse(' acme ').data).toBe('acme');
    expect(prnField.safeParse('p'.repeat(512)).success).toBe(true);
    expect(prnField.safeParse('p'.repeat(513)).success).toBe(false);
    expect(prnField.safeParse('  ').success).toBe(false);
  });

  it('accepts an empty current value, and a current name longer than the name bound', () => {
    expect(currentField.safeParse('').success).toBe(true);
    expect(currentField.safeParse('a'.repeat(300)).success).toBe(true);
    expect(currentField.safeParse(null).success).toBe(false);
  });
});

// SMA-630 spec D6, § 4.3. A rename sends only the fields that the user changed.
describe('renameChange', () => {
  const current = { currentSlug: 'acme', currentName: 'Acme' };

  it('sends only the slug when only the slug changed', () => {
    expect(renameChange({ ...current, slug: 'acme-2', name: 'Acme' })).toEqual({ newSlug: 'acme-2' });
  });

  it('sends only the name when only the name changed', () => {
    expect(renameChange({ ...current, slug: 'acme', name: 'Acme Two' })).toEqual({ newName: 'Acme Two' });
  });

  it('sends both when both changed', () => {
    expect(renameChange({ ...current, slug: 'acme-2', name: 'Acme Two' })).toEqual({ newSlug: 'acme-2', newName: 'Acme Two' });
  });

  it('sends neither field when nothing changed, so IAM answers nothing-to-rename', () => {
    const change = renameChange({ ...current, slug: 'acme', name: 'Acme' });
    expect(change).toEqual({});
    expect('newSlug' in change).toBe(false);
    expect('newName' in change).toBe(false);
  });

  it('compares trimmed values on both sides', () => {
    expect(renameChange({ slug: ' acme ', name: ' Acme ', currentSlug: 'acme', currentName: 'Acme' })).toEqual({});
    expect(renameChange({ slug: 'acme', name: 'Acme', currentSlug: ' acme', currentName: 'Acme ' })).toEqual({});
    expect(renameChange({ slug: ' new ', name: 'Acme', currentSlug: 'acme', currentName: 'Acme' })).toEqual({ newSlug: 'new' });
  });
});
