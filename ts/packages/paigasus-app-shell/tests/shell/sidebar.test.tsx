// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { NavEntry } from '../../src/nav/primary-nav';
import { AppShell } from '../../src/shell/app-shell';
import { WIDE_QUERY } from '../../src/shell/sidebar';
import { setPathname } from '../support/next-navigation-double';
import { inShell } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));
vi.mock('next/navigation', () => import('../support/next-navigation-double'));

const BRAND = { label: 'IAM', href: '/iam' };
const NAV: readonly NavEntry[] = [
  { zone: 'iam', href: '/iam/users', label: 'Users', state: { state: 'available' }, group: 'Directory' },
  { zone: 'iam', href: '/iam/audit', label: 'Audit', state: { state: 'available' }, group: 'Administration' },
];

/** jsdom has no matchMedia; a test that needs a narrow screen installs one that answers `wide`. */
function setViewport(wide: boolean): void {
  vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === WIDE_QUERY ? wide : false, media: query, addEventListener: () => undefined, removeEventListener: () => undefined }));
}

function renderShell(nav: readonly NavEntry[] = NAV): void {
  setPathname('/users');
  render(
    inShell(
      <AppShell brand={BRAND} nav={nav}>
        <p>page content</p>
      </AppShell>,
    ),
  );
}

const toggle = (): HTMLElement => screen.getByRole('button', { name: 'Navigation' });
const sidebar = (): HTMLElement => {
  const id = toggle().getAttribute('aria-controls');
  const element = id === null ? null : document.getElementById(id);
  if (element === null) throw new Error('the toggle controls no element');
  return element;
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the collapsible sidebar (SMA-783)', () => {
  it('a toggle in the banner controls the element holding the Primary nav', () => {
    renderShell();
    expect(screen.getByRole('banner')).toContainElement(toggle());
    expect(sidebar()).toContainElement(screen.getByRole('navigation', { name: 'Primary' }));
  });

  it('wide screen: open by default; the toggle collapses it and opens it again', async () => {
    const user = userEvent.setup();
    setViewport(true);
    renderShell();
    expect(toggle()).toHaveAttribute('aria-expanded', 'true');
    // No choice yet: CSS shows it from md up and hides it below, so a narrow screen never flashes it.
    expect(sidebar()).toHaveClass('hidden', 'md:flex');
    await user.click(toggle());
    expect(toggle()).toHaveAttribute('aria-expanded', 'false');
    expect(sidebar().className).toBe('hidden');
    await user.click(toggle());
    expect(toggle()).toHaveAttribute('aria-expanded', 'true');
    expect(sidebar()).toHaveClass('flex');
    expect(sidebar()).not.toHaveClass('hidden');
  });

  it('narrow screen: collapsed by default, and opens over main', async () => {
    const user = userEvent.setup();
    setViewport(false);
    renderShell();
    expect(toggle()).toHaveAttribute('aria-expanded', 'false');
    await user.click(toggle());
    expect(toggle()).toHaveAttribute('aria-expanded', 'true');
    expect(sidebar()).toHaveClass('flex', 'max-md:absolute', 'max-md:z-40');
  });

  it('narrow screen: Escape inside the open sidebar closes it and returns focus to the toggle', async () => {
    const user = userEvent.setup();
    setViewport(false);
    renderShell();
    await user.click(toggle());
    screen.getByRole('link', { name: 'Audit' }).focus();
    await user.keyboard('{Escape}');
    expect(toggle()).toHaveAttribute('aria-expanded', 'false');
    expect(toggle()).toHaveFocus();
  });

  it('narrow screen: following a link closes the sidebar', async () => {
    const user = userEvent.setup();
    setViewport(false);
    renderShell();
    await user.click(toggle());
    await user.click(screen.getByRole('link', { name: 'Audit' }));
    expect(toggle()).toHaveAttribute('aria-expanded', 'false');
  });

  it('wide screen: Escape and links leave the sidebar open (it covers nothing)', async () => {
    const user = userEvent.setup();
    setViewport(true);
    renderShell();
    await user.click(screen.getByRole('link', { name: 'Audit' }));
    screen.getByRole('link', { name: 'Users' }).focus();
    await user.keyboard('{Escape}');
    expect(toggle()).toHaveAttribute('aria-expanded', 'true');
  });

  it('has no toggle when no nav entry is visible', () => {
    renderShell([{ ...NAV[0], state: { state: 'absent' } } as NavEntry]);
    expect(screen.queryByRole('button', { name: 'Navigation' })).not.toBeInTheDocument();
    expect(screen.queryByRole('navigation', { name: 'Primary' })).not.toBeInTheDocument();
  });
});
