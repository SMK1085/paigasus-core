// SPDX-License-Identifier: Apache-2.0
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Switcher, type SwitcherItem } from '../../src/shell/switcher';
import { nextLinkClicks } from '../support/next-link-double';
import { inZone } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));

const ACME: SwitcherItem = { id: 'o1', label: 'Acme', href: '/iam/orgs/o1' };
const GLOBEX: SwitcherItem = { id: 'o2', label: 'Globex', href: '/iam/orgs/o2' };
// Matches no zone in ZONES ({ iam, gateway }), so resolveZone returns null for it.
const ELSEWHERE: SwitcherItem = { id: 'x1', label: 'Elsewhere', href: '/billing/x1' };

beforeEach(() => {
  nextLinkClicks.length = 0;
});

describe('Switcher (spec § 8.3)', () => {
  it("the trigger's visible text is `${label}: ${current}`, and it is also the accessible name (WCAG 2.5.3)", () => {
    render(inZone(<Switcher label="Organisation" items={[ACME, GLOBEX]} currentId="o2" />));
    const trigger = screen.getByRole('button', { name: 'Organisation: Globex' });
    expect(trigger).toHaveTextContent('Organisation: Globex');
  });

  it('says "none selected" when currentId is null', () => {
    render(inZone(<Switcher label="Organisation" items={[ACME]} currentId={null} />));
    expect(screen.getByRole('button', { name: 'Organisation: none selected' })).toBeInTheDocument();
  });

  it('says "none selected" when currentId names an item that the filter removed', () => {
    render(inZone(<Switcher label="Organisation" items={[ELSEWHERE, ACME]} currentId="x1" />));
    expect(screen.getByRole('button', { name: 'Organisation: none selected' })).toBeInTheDocument();
  });

  it('does not render an unmatched item', async () => {
    const user = userEvent.setup();
    render(inZone(<Switcher label="Organisation" items={[ACME, ELSEWHERE]} currentId={null} />));
    await user.click(screen.getByRole('button', { name: 'Organisation: none selected' }));
    expect(screen.getAllByRole('menuitem').map((item) => item.textContent)).toEqual(['Acme']);
  });

  it('F18: with an unmatched item FIRST, keyboard open focuses the first REMAINING item and does not throw', async () => {
    // Radix keeps a DropdownMenuItem registered when its asChild child renders null, and keyboard
    // open then calls .focus() on null (F18). The filter-first design is what keeps this green.
    // With a mouse open Radix skips entry focus, so ONLY the keyboard path exposes the defect.
    const user = userEvent.setup();
    render(inZone(<Switcher label="Organisation" items={[ELSEWHERE, ACME, GLOBEX]} currentId="o1" />));
    screen.getByRole('button', { name: 'Organisation: Acme' }).focus();
    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'Acme' })).toHaveFocus();
    });
  });

  it('renders nothing when every item is unmatched', () => {
    const { container } = render(inZone(<Switcher label="Organisation" items={[ELSEWHERE]} currentId={null} />));
    expect(container).toBeEmptyDOMElement();
  });

  it('marks the current item with aria-current="true" and a visible check mark, and no other item', async () => {
    const user = userEvent.setup();
    render(inZone(<Switcher label="Organisation" items={[ACME, GLOBEX]} currentId="o1" />));
    await user.click(screen.getByRole('button', { name: 'Organisation: Acme' }));
    const acme = screen.getByRole('menuitem', { name: 'Acme' });
    const globex = screen.getByRole('menuitem', { name: 'Globex' });
    expect(acme).toHaveAttribute('aria-current', 'true');
    expect(acme).toHaveTextContent('✓');
    expect(globex).not.toHaveAttribute('aria-current');
    expect(globex).not.toHaveTextContent('✓');
  });

  it('choosing an item is a navigation: the URL owns the selection (D1)', async () => {
    const user = userEvent.setup();
    render(inZone(<Switcher label="Organisation" items={[ACME, GLOBEX]} currentId="o1" />));
    await user.click(screen.getByRole('button', { name: 'Organisation: Acme' }));
    await user.click(screen.getByRole('menuitem', { name: 'Globex' }));
    expect(nextLinkClicks).toEqual(['/orgs/o2']);
    // ZoneLinkProps omits onClick/onMouseEnter/onTouchStart at the type level, but ZoneLink spreads
    // ...anchorProps onto the rendered element, so the handler Radix's Slot injects must still reach
    // the DOM at runtime. Radix closes the menu only when its own item handler runs, so a closed
    // menu after the click proves the handler reached the element.
    await waitFor(() => {
      expect(screen.queryByRole('menu')).toBeNull();
    });
  });

  it('keyboard: opening with the keyboard, ArrowDown to Globex, then Enter also closes the menu', async () => {
    const user = userEvent.setup();
    render(inZone(<Switcher label="Organisation" items={[ACME, GLOBEX]} currentId="o1" />));
    screen.getByRole('button', { name: 'Organisation: Acme' }).focus();
    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'Acme' })).toHaveFocus();
    });
    await user.keyboard('{ArrowDown}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'Globex' })).toHaveFocus();
    });
    await user.keyboard('{Enter}');
    expect(nextLinkClicks).toEqual(['/orgs/o2']);
    await waitFor(() => {
      expect(screen.queryByRole('menu')).toBeNull();
    });
  });
});
