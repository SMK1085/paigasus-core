// SPDX-License-Identifier: Apache-2.0
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';

describe('expectNoAxeViolations', () => {
  // The helper's own negative control. A helper that silently stopped running (a wrong tag set, a
  // swallowed promise, an empty target) would look like a clean pass on every test in the package.
  // The rule id is matched in its formatted position (`[impact] id: help`), as in @paigasus/ui.
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

  // `region` is a best-practice rule, OUTSIDE the WCAG tag set the helper runs. These rows prove
  // that `{ region: true }` really adds it to the run (spec § 10.2) — without them, the shell's
  // landmark assertion could be a no-op that nothing notices.
  it('with region ON, rejects content outside every landmark', async () => {
    render(<p>stray text</p>);
    await expect(expectNoAxeViolations(document.body, { region: true })).rejects.toThrow(/\] region: /);
  });

  it('with region ON, resolves when the content sits in a landmark', async () => {
    render(
      <main>
        <p>inside</p>
      </main>,
    );
    await expect(expectNoAxeViolations(document.body, { region: true })).resolves.toBeUndefined();
  });

  it('with region OFF (the default), ignores content outside landmarks', async () => {
    render(<p>stray text</p>);
    await expect(expectNoAxeViolations()).resolves.toBeUndefined();
  });
});
