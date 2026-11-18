// SPDX-License-Identifier: Apache-2.0
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { OrganizationIcon } from '../../src/shell/icons';
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

  it('with an icon, shows the icon (hidden from assistive tech) in place of the visible prefix, and keeps `${label}: ${current}` as the accessible name', () => {
    render(inZone(<Switcher label="Organisation" icon={<OrganizationIcon />} items={[ACME, GLOBEX]} currentId="o2" />));
    const trigger = screen.getByRole('button', { name: 'Organisation: Globex' });
    const icon = trigger.querySelector('svg');
    expect(icon).not.toBeNull();
    expect(icon?.closest('[aria-hidden="true"]')).not.toBeNull();
    // The prefix is screen-reader text only: the one visible text is the current item.
    const visible = [...trigger.querySelectorAll('span')].filter((span) => !span.classList.contains('sr-only') && span.getAttribute('aria-hidden') !== 'true');
    expect(visible.map((span) => span.textContent)).toEqual(['Globex']);
    expect(trigger.querySelector('.sr-only')).toHaveTextContent('Organisation:');
    expect(trigger).toHaveTextContent('Organisation: Globex');
  });

  it("with initials, shows a square badge of the current item's initials, truncates the name and hides it below md (SMA-783)", () => {
    const NORTHWIND: SwitcherItem = { id: 'o3', label: 'Northwind Labs', href: '/iam/orgs/o3' };
    render(inZone(<Switcher label="Organisation" initials icon={<OrganizationIcon />} items={[ACME, NORTHWIND]} currentId="o3" />));
    const trigger = screen.getByRole('button', { name: 'Organisation: Northwind Labs' });
    expect(trigger).toHaveAttribute('title', 'Organisation: Northwind Labs');
    const badge = trigger.querySelector('[aria-hidden="true"]');
    expect(badge).toHaveTextContent('NL');
    expect(badge).toHaveClass('rounded-xs');
    // The badge takes the icon's place.
    expect(trigger.querySelector('svg')).toBeNull();
    const name = screen.getByText('Northwind Labs');
    expect(name).toHaveClass('truncate', 'max-w-48', 'max-md:sr-only');
  });

  it('with initials and nothing selected, falls back to the icon and shows "none selected" in full', () => {
    render(inZone(<Switcher label="Organisation" initials icon={<OrganizationIcon />} items={[ACME, GLOBEX]} currentId={null} />));
    const trigger = screen.getByRole('button', { name: 'Organisation: none selected' });
    expect(trigger.querySelector('svg')).not.toBeNull();
    expect(screen.getByText('none selected')).not.toHaveClass('max-md:sr-only');
  });

  it('without icon or initials, has no tooltip (the visible text is the whole name)', () => {
    render(inZone(<Switcher label="Organisation" items={[ACME, GLOBEX]} currentId="o2" />));
    expect(screen.getByRole('button', { name: 'Organisation: Globex' })).not.toHaveAttribute('title');
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
