// SPDX-License-Identifier: Apache-2.0
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';

describe('expectNoAxeViolations', () => {
  /*
   * The helper's own negative control. Without this, a helper that silently stopped running —
   * a wrong tag set, a swallowed promise, an empty target — would look identical to a clean
   * pass on every component test in the package.
   */
  it('rejects a control with no accessible name', async () => {
    render(<input type="text" />);
    await expect(expectNoAxeViolations()).rejects.toThrow(/axe found 1 violation/);
  });

  it('resolves for a correctly labelled control', async () => {
    render(
      <>
        <label htmlFor="probe">Probe</label>
        <input id="probe" type="text" />
      </>,
    );
    await expect(expectNoAxeViolations()).resolves.toBeUndefined();
  });
});
