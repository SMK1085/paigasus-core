// SPDX-License-Identifier: Apache-2.0
import type { ReactElement } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Combobox } from '../src/components/combobox';

const ITEMS = [
  { value: 'iam', label: 'IAM' },
  { value: 'gateway', label: 'Gateway' },
];

describe('Combobox', () => {
  it('opens, filters and reports the chosen value', async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    render(<Combobox items={ITEMS} placeholder="Select a service" onValueChange={onValueChange} />);

    // Uncontrolled and nothing selected yet, so the input starts empty — no clear needed here.
    // The controlled tests below do need one, and say so at their own call sites.
    await user.click(screen.getByRole('combobox'));
    await user.type(screen.getByRole('combobox'), 'gate');
    await user.click(screen.getByRole('option', { name: 'Gateway' }));

    expect(onValueChange).toHaveBeenCalledWith('gateway');
  });

  it('shows the empty message when nothing matches', async () => {
    const user = userEvent.setup();
    render(<Combobox items={ITEMS} emptyMessage="No service found." />);
    await user.click(screen.getByRole('combobox'));
    await user.type(screen.getByRole('combobox'), 'zzz');
    expect(screen.getByText('No service found.')).toBeInTheDocument();
  });

  it('has no axe violations while open', async () => {
    const user = userEvent.setup();
    render(<Combobox items={ITEMS} placeholder="Select a service" />);
    await user.click(screen.getByRole('combobox'));
    await expectNoAxeViolations();
  });

  it('merges className onto the outermost element', () => {
    const { container } = render(<Combobox items={ITEMS} className="w-96" />);
    expect(container.firstElementChild).toHaveClass('w-96');
    // The base classes survive the merge — className adds, it does not replace.
    expect(container.firstElementChild).toHaveClass('bg-popover');
  });

  /*
   * SMA-503 fix round 2, item 1. `useState(selectedLabel)` read its initializer once, so a
   * controlled `value` that moved after mount never reached the input. Both tests below fail
   * against that implementation.
   */
  it('repaints when a controlled value changes after mount', () => {
    const { rerender } = render(<Combobox items={ITEMS} value="iam" />);
    expect(screen.getByRole('combobox')).toHaveValue('IAM');

    rerender(<Combobox items={ITEMS} value="gateway" />);
    expect(screen.getByRole('combobox')).toHaveValue('Gateway');

    // Clearing the value (a form reset, a failed save) must clear the input too.
    rerender(<Combobox items={ITEMS} value="" />);
    expect(screen.getByRole('combobox')).toHaveValue('');
  });

  it('does not paint a label a controlled parent declined', async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();

    // A parent that reports the change and keeps its own value — a rejected edit.
    function Declining(): ReactElement {
      return <Combobox items={ITEMS} value="iam" onValueChange={onValueChange} />;
    }
    render(<Declining />);
    expect(screen.getByRole('combobox')).toHaveValue('IAM');

    // Clear first: the input already shows 'IAM', and typing appends rather than replaces.
    await user.click(screen.getByRole('combobox'));
    await user.clear(screen.getByRole('combobox'));
    await user.type(screen.getByRole('combobox'), 'gate');
    await user.click(screen.getByRole('option', { name: 'Gateway' }));

    expect(onValueChange).toHaveBeenCalledWith('gateway');
    // The parent declined, so the input must fall back to the value it still holds — not to
    // the rejected label, and not to the half-typed filter string either.
    expect(screen.getByRole('combobox')).toHaveValue('IAM');
  });

  /*
   * SMA-503 local review, finding B. `shouldFilter` was unconditionally true and `search`
   * falls back to the selected item's label, so reopening after a selection made cmdk filter
   * on that label and the list collapsed to the one item already chosen.
   */
  it('lists every item again after a selection is made and the popover reopens', async () => {
    const user = userEvent.setup();
    render(<Combobox items={ITEMS} placeholder="Select a service" />);

    await user.click(screen.getByRole('combobox'));
    await user.click(screen.getByRole('option', { name: 'IAM' }));
    expect(screen.getByRole('combobox')).toHaveValue('IAM');

    // Reopen. The input still holds 'IAM', but that is a SELECTION, not a filter — every item
    // must still be reachable or the user cannot change their mind.
    await user.click(screen.getByRole('combobox'));
    expect(screen.getByRole('option', { name: 'IAM' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'Gateway' })).toBeInTheDocument();
  });

  /*
   * SMA-503 local review, finding C. onOpenChange only tracked `open`, so a draft abandoned by
   * dismissing the popover survived and the input kept showing filter text that matched
   * nothing the component had selected.
   */
  it('drops an abandoned filter when the popover closes without a selection', async () => {
    const user = userEvent.setup();
    render(<Combobox items={ITEMS} value="iam" />);
    expect(screen.getByRole('combobox')).toHaveValue('IAM');

    await user.click(screen.getByRole('combobox'));
    await user.clear(screen.getByRole('combobox'));
    await user.type(screen.getByRole('combobox'), 'gate');
    expect(screen.getByRole('combobox')).toHaveValue('gate');

    // Dismiss without choosing anything.
    await user.keyboard('{Escape}');
    expect(screen.getByRole('combobox')).toHaveValue('IAM');
  });

  /*
   * SMA-503 local review, finding D. cmdk normalises the value it hands to onSelect. MEASURED
   * on the pinned cmdk 1.1.1: it TRIMS and does NOT lower-case (`useValue` stores
   * `value.trim()`; the only toLowerCase in the package is inside command-score, which scores
   * a match and never touches the stored value). So the uppercase case below passes even
   * without the fix and the whitespace case does not — both are kept, because the fix must
   * hold whichever way a future cmdk normalises.
   */
  it("reports the raw item value, not cmdk's normalised one", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();
    const rawItems = [
      { value: 'IAM', label: 'Identity' },
      { value: 'gateway ', label: 'Gateway' },
    ];
    render(<Combobox items={rawItems} onValueChange={onValueChange} />);

    await user.click(screen.getByRole('combobox'));
    await user.click(screen.getByRole('option', { name: 'Identity' }));
    expect(onValueChange).toHaveBeenLastCalledWith('IAM');
    // The raw value round-trips, so the label lookup still resolves.
    expect(screen.getByRole('combobox')).toHaveValue('Identity');

    await user.click(screen.getByRole('combobox'));
    await user.click(screen.getByRole('option', { name: 'Gateway' }));
    expect(onValueChange).toHaveBeenLastCalledWith('gateway ');
    expect(screen.getByRole('combobox')).toHaveValue('Gateway');
  });
});
