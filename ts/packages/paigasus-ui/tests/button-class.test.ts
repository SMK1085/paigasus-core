// SPDX-License-Identifier: Apache-2.0
//
// SMA-636 D11: the two button classes the console forms share live here. The exact strings are the
// ones iam-console used, so moving them changes no rendered class.
import { describe, expect, it } from 'vitest';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '../src/index';

describe('the shared button classes', () => {
  it('are exported from the package entry with the classes iam-console used', () => {
    expect(PRIMARY_BUTTON_CLASS).toBe('bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50');
    expect(SECONDARY_BUTTON_CLASS).toBe('border-input rounded-pgs self-start border px-3 py-1.5 text-sm font-medium disabled:opacity-50');
  });
});
