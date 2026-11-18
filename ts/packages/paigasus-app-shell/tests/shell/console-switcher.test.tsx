// SPDX-License-Identifier: Apache-2.0
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { NavEntry } from '../../src/nav/rows';
import { ConsoleSwitcher } from '../../src/shell/console-switcher';
import { nextLinkClicks } from '../support/next-link-double';
import { SESSION, inShell } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));

const BRAND = { label: 'IAM', href: '/iam/orgs' };
const IAM: NavEntry = { zone: 'iam', href: '/iam/orgs', label: 'IAM', state: { state: 'available' } };
const GATEWAY: NavEntry = { zone: 'gateway', href: '/gateway/overview', label: 'AI Gateway', state: { state: 'available' } };
const BILLING: NavEntry = { zone: 'billing', href: '/billing/invoices', label: 'Billing', state: { state: 'available' } };
const TRIGGER = 'Switch console: Paigasus IAM';

beforeEach(() => {
  nextLinkClicks.length = 0;
});

describe('ConsoleSwitcher (SMA-783)', () => {
  it('is a menu button whose name keeps the visible label, with the logos and the grid glyph decorative', () => {
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, GATEWAY]} />));
    const trigger = screen.getByRole('button', { name: TRIGGER });
    expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
    expect([...trigger.querySelectorAll('img')].map((img) => img.getAttribute('alt'))).toEqual(['', '']);
    expect(trigger.querySelector('svg')?.closest('[aria-hidden="true"]')).not.toBeNull();
  });

  it('lists every console, ticks the current one with aria-current, and links another console with a plain <a>', async () => {
    const user = userEvent.setup();
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, GATEWAY]} />));
    await user.click(screen.getByRole('button', { name: TRIGGER }));
    const menu = screen.getByRole('menu');
    expect(within(menu).getByText('Consoles')).toBeInTheDocument();
    const iam = screen.getByRole('menuitem', { name: 'IAM' });
    const gateway = screen.getByRole('menuitem', { name: 'AI Gateway' });
    expect(iam).toHaveAttribute('aria-current', 'true');
    expect(iam).toHaveTextContent('✓IAM');
    expect(gateway).not.toHaveAttribute('aria-current');
    expect(gateway).not.toHaveAttribute('data-next-link');
    expect(gateway).toHaveAttribute('href', '/gateway/overview');
  });

  it('opens from the keyboard and Escape returns focus to the trigger', async () => {
    const user = userEvent.setup();
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, GATEWAY]} />));
    const trigger = screen.getByRole('button', { name: TRIGGER });
    trigger.focus();
    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'IAM' })).toHaveFocus();
    });
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });

  it('the current console navigates inside the zone (next/link), to its own home', async () => {
    const user = userEvent.setup();
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, GATEWAY]} />));
    await user.click(screen.getByRole('button', { name: TRIGGER }));
    await user.click(screen.getByRole('menuitem', { name: 'IAM' }));
    expect(nextLinkClicks).toEqual(['/orgs']);
  });

  it('shows a degraded console as a disabled item with its reason, and hides an absent one', async () => {
    const user = userEvent.setup();
    const degraded: NavEntry = { ...GATEWAY, state: { state: 'degraded', service: 'AI Gateway', reason: 'timeout' } };
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, degraded, { ...BILLING, zone: 'iam', href: '/iam/billing', state: { state: 'absent' } }]} />));
    await user.click(screen.getByRole('button', { name: TRIGGER }));
    const items = screen.getAllByRole('menuitem');
    expect(items).toHaveLength(2);
    expect(items[1]).toHaveAttribute('aria-disabled', 'true');
    expect(items[1]).toHaveTextContent('AI Gateway is not answering (timed out)');
    expect(items[1]?.querySelector('a')).toBeNull();
    // Readable: the reason is not dimmed by the menu's disabled opacity.
    expect(items[1]?.className).toContain('data-[disabled]:opacity-100');
    expect(items[1]?.className).not.toContain('data-[disabled]:opacity-50');
  });

  it('is the plain brand link when fewer than two consoles remain after the entry rules', () => {
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, BILLING, { ...GATEWAY, state: { state: 'absent' } }]} />));
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Paigasus IAM' })).toBeInTheDocument();
  });

  it('hides a console the session may not use (`requires`, cosmetic only)', () => {
    const admin = { scopePrn: 'prn:paigasus:iam::org/o1', roleKey: 'org.admin' };
    render(inShell(<ConsoleSwitcher brand={BRAND} consoles={[IAM, { ...GATEWAY, requires: admin }]} />, { session: SESSION }));
    expect(screen.getByRole('link', { name: 'Paigasus IAM' })).toBeInTheDocument();
  });
});
