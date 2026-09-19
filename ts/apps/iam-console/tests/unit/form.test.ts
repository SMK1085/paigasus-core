// SPDX-License-Identifier: Apache-2.0
//
// The rename helpers that stay in this zone. The shared helpers and their tests moved to
// @paigasus/console-core's tests/unit/form.test.ts (SMA-636 D11).
import { describe, expect, it } from 'vitest';
import type { Presentation } from '@paigasus/sdk/errors/types';
import { invalidFormInput } from '@paigasus/console-core';
import { currentField, refreshesAfterLifecycleAction, renameChange, renameForm, slugField } from '../../lib/form';

describe('the rename field bounds', () => {
  it('keeps the slug bound at 200, trimmed', () => {
    expect(slugField.safeParse('s'.repeat(200)).success).toBe(true);
    expect(slugField.safeParse('s'.repeat(201)).success).toBe(false);
    expect(slugField.safeParse(' acme ').data).toBe('acme');
  });

  it('accepts an empty current value, and a current name longer than the name bound', () => {
    expect(currentField.safeParse('').success).toBe(true);
    expect(currentField.safeParse('a'.repeat(300)).success).toBe(true);
    expect(currentField.safeParse(null).success).toBe(false);
  });

  it('carries no max of its own: a current value can pass 4096 characters (CR round 1)', () => {
    expect(currentField.safeParse('a'.repeat(5000)).success).toBe(true);
  });
});

// SMA-630 CR round 1, spec § 4.2, updated by SMA-642. The `name` bound applies only when the
// trimmed name changed: IAM holds names longer than 256 code points that it stored before SMA-642
// validated the rename path, it does not migrate them (SMA-642 D4), and a slug-only rename of such
// a node must still work.
describe('renameForm (the shared rename schema)', () => {
  const form = renameForm();
  const base = { prn: 'prn:pgs:iam::o:org/o', slug: 'acme', name: 'Acme', currentSlug: 'acme-old', currentName: 'Acme' };
  const longName = 'a'.repeat(300);

  it('parses a slug-only rename of a node whose stored name is 300 code points (unchanged)', () => {
    const result = form.safeParse({ ...base, slug: 'acme-2', name: longName, currentName: longName });
    expect(result.success).toBe(true);
  });

  it('refuses a CHANGED name of 300 code points', () => {
    const result = form.safeParse({ ...base, name: longName, currentName: 'Acme' });
    expect(result.success).toBe(false);
  });

  it('parses a slug-only rename of a node whose stored name is longer than 4096 characters (unchanged)', () => {
    const veryLongName = 'a'.repeat(5000);
    const result = form.safeParse({ ...base, slug: 'acme-2', name: veryLongName, currentName: veryLongName });
    expect(result.success).toBe(true);
  });

  it('still refuses an empty CHANGED name', () => {
    const result = form.safeParse({ ...base, name: '', currentName: 'Acme' });
    expect(result.success).toBe(false);
  });

  it('accepts an unchanged name equal to the bound and refuses a changed one past it', () => {
    expect(form.safeParse({ ...base, name: 'Acme', currentName: 'Acme' }).success).toBe(true);
    expect(form.safeParse({ ...base, slug: 'acme-2', name: 'a'.repeat(257), currentName: 'a'.repeat(257) }).success).toBe(true);
    expect(form.safeParse({ ...base, name: 'a'.repeat(257), currentName: 'Acme' }).success).toBe(false);
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

// SMA-630 spec § 4.4. A lifecycle action refreshes on success, and ALSO on the two refusals that
// often mean the page is stale.
describe('refreshesAfterLifecycleAction', () => {
  const refused = (presentation: Presentation) => ({ ok: false as const, error: { ...invalidFormInput(), presentation } });

  it('refreshes on a success', () => {
    expect(refreshesAfterLifecycleAction({ ok: true })).toBe(true);
  });

  it.each<[Presentation, boolean]>([
    ['forbidden', true],
    ['conflict', true],
    ['invalid-input', false],
    ['relogin', false],
    ['not-found', false],
    ['degraded', false],
    ['rate-limited', false],
    ['disabled', false],
    ['generic', false],
  ])('a refusal with presentation %s refreshes: %s', (presentation, expected) => {
    expect(refreshesAfterLifecycleAction(refused(presentation))).toBe(expected);
  });
});
