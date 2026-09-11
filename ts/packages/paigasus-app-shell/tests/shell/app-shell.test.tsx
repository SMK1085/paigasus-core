// SPDX-License-Identifier: Apache-2.0
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { UseSessionOutsideProviderError } from '@paigasus/auth/client';
import type { NavEntry } from '../../src/nav/primary-nav';
import { AppShell } from '../../src/shell/app-shell';
import { Breadcrumbs } from '../../src/shell/breadcrumbs';
import { PublicShell } from '../../src/shell/public-shell';
import type { SwitcherProps } from '../../src/shell/switcher';
import { expectNoAxeViolations } from '../axe';
import { silenceReactErrorLog } from '../support/console';
import { nextLinkClicks } from '../support/next-link-double';
import { setPathname } from '../support/next-navigation-double';
import { inShell, inZone } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));
vi.mock('next/navigation', () => import('../support/next-navigation-double'));

const BRAND = { label: 'Paigasus', href: '/iam' };
const NAV: readonly NavEntry[] = [
  { zone: 'iam', href: '/iam/users', label: 'Users', state: { state: 'available' } },
  { zone: 'gateway', href: '/gateway/usage', label: 'Gateway', state: { state: 'degraded', service: 'gateway', reason: 'timeout' } },
];
const ORGS: SwitcherProps = {
  label: 'Organisation',
  items: [
    { id: 'o1', label: 'Acme', href: '/iam/orgs/o1' },
    { id: 'o2', label: 'Globex', href: '/iam/orgs/o2' },
  ],
  currentId: 'o1',
};

function Shell() {
  return (
    <AppShell brand={BRAND} nav={NAV} switchers={[ORGS]}>
      <p>page content</p>
    </AppShell>
  );
}

beforeEach(() => {
  setPathname('/users');
  nextLinkClicks.length = 0;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('AppShell (spec § 8.1)', () => {
  it('renders a skip link, then a banner with the brand, the Primary nav, the switcher and the user menu, then main', () => {
    render(inShell(<Shell />));
    const skip = screen.getByRole('link', { name: 'Skip to content' });
    expect(skip).toHaveAttribute('href', '#main');
    const banner = screen.getByRole('banner');
    expect(within(banner).getByRole('link', { name: 'Paigasus' })).toBeInTheDocument();
    expect(within(banner).getByRole('navigation', { name: 'Primary' })).toBeInTheDocument();
    expect(within(banner).getByRole('button', { name: 'Organisation: Acme' })).toBeInTheDocument();
    expect(within(banner).getByRole('button', { name: 'Ada Lovelace' })).toBeInTheDocument();
    const main = screen.getByRole('main');
    expect(main).toHaveAttribute('id', 'main');
    expect(main).toHaveAttribute('tabindex', '-1');
    expect(main).toHaveTextContent('page content');
  });

  it('throws UseSessionOutsideProviderError with no SessionProvider: a missing provider is loud (F6)', () => {
    silenceReactErrorLog();
    expect(() => render(inZone(<Shell />))).toThrow(UseSessionOutsideProviderError);
  });
});

describe('PublicShell (spec § 8.2, AC 4)', () => {
  it('renders with NO SessionProvider and does not throw', () => {
    expect(() =>
      render(
        inZone(
          <PublicShell brand={BRAND}>
            <p>welcome</p>
          </PublicShell>,
        ),
      ),
    ).not.toThrow();
  });

  it('has no Primary nav and no account menu', () => {
    render(
      inZone(
        <PublicShell brand={BRAND}>
          <p>welcome</p>
        </PublicShell>,
      ),
    );
    expect(screen.queryByRole('navigation', { name: 'Primary' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('"Sign in" is a plain <a> to the zone login route, never next/link (spec § 6.5)', () => {
    render(
      inZone(
        <PublicShell brand={BRAND}>
          <p>welcome</p>
        </PublicShell>,
      ),
    );
    const signIn = screen.getByRole('link', { name: 'Sign in' });
    expect(signIn).toHaveAttribute('href', '/iam/auth/login');
    expect(signIn).not.toHaveAttribute('data-next-link');
  });
});

describe('axe (AC 5)', () => {
  it('the closed AppShell, which includes a degraded entry — region rule ON', async () => {
    render(inShell(<Shell />));
    await expectNoAxeViolations(document.body, { region: true });
  });

  it('the AppShell with the switcher open', async () => {
    const user = userEvent.setup();
    render(inShell(<Shell />));
    await user.click(screen.getByRole('button', { name: 'Organisation: Acme' }));
    expect(screen.getByRole('menu')).toBeInTheDocument();
    await expectNoAxeViolations();
  });

  it('the AppShell with the user menu open', async () => {
    const user = userEvent.setup();
    render(inShell(<Shell />));
    await user.click(screen.getByRole('button', { name: 'Ada Lovelace' }));
    expect(screen.getByRole('menu')).toBeInTheDocument();
    await expectNoAxeViolations();
  });

  it('the PublicShell — region rule ON', async () => {
    render(
      inZone(
        <PublicShell brand={BRAND}>
          <p>welcome</p>
        </PublicShell>,
      ),
    );
    await expectNoAxeViolations(document.body, { region: true });
  });

  it('a page trail inside the shell', async () => {
    render(
      inShell(
        <AppShell brand={BRAND} nav={NAV}>
          <Breadcrumbs items={[{ label: 'Users', href: '/iam/users' }, { label: 'Ada' }]} />
        </AppShell>,
      ),
    );
    await expectNoAxeViolations(document.body, { region: true });
  });
});

describe('the keyboard script (AC 5, spec § 10.2)', () => {
  it('Tab order, Enter to open, ArrowDown, Escape back to the trigger, Enter to activate', async () => {
    const user = userEvent.setup();
    render(inShell(<Shell />));

    await user.tab();
    expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('link', { name: 'Paigasus' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('link', { name: 'Users' })).toHaveFocus();
    await user.tab();
    // The degraded entry is in the tab order (spec § 7.3).
    expect(screen.getByRole('link', { name: 'Gateway' })).toHaveFocus();
    await user.tab();
    const trigger = screen.getByRole('button', { name: 'Organisation: Acme' });
    expect(trigger).toHaveFocus();

    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'Acme' })).toHaveFocus();
    });
    await user.keyboard('{ArrowDown}');
    expect(screen.getByRole('menuitem', { name: 'Globex' })).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();

    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'Acme' })).toHaveFocus();
    });
    await user.keyboard('{ArrowDown}');
    await user.keyboard('{Enter}');
    expect(nextLinkClicks).toEqual(['/orgs/o2']);
  });
});
