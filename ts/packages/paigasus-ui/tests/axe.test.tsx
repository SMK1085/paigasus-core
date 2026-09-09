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
  /*
   * Match the RULE ID, not the violation count. `axe-core` is a `^4.13.0` catalog range that
   * Dependabot bumps, and a future version can change how many violations one unlabelled input
   * produces — which would red this test for a reason unrelated to the control. Worse, a count
   * match does not prove the missing-accessible-name rule is what fired: any other single
   * violation would satisfy it just as well.
   *
   * The id is pinned in its formatted position (`[impact] id: help`, see tests/axe.ts) rather
   * than as a bare word, because the rule's own helpUrl ends in `/label` and a loose `\blabel\b`
   * would match that even if a different rule had fired.
   */
  it('rejects a control with no accessible name', async () => {
    render(<input type="text" />);
    await expect(expectNoAxeViolations()).rejects.toThrow(/\] label: /);
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
