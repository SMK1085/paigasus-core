// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PrimaryNav, type NavEntry } from '../../src/nav/primary-nav';
import type { NavEntryState } from '../../src/nav/state';
import { ZoneLinkError } from '../../src/zone/errors';
import { silenceReactErrorLog } from '../support/console';
import { setPathname } from '../support/next-navigation-double';
import { SESSION, ZONES, inShell } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));
vi.mock('next/navigation', () => import('../support/next-navigation-double'));

const AVAILABLE: NavEntryState = { state: 'available' };
const ABSENT: NavEntryState = { state: 'absent' };
const DEGRADED: NavEntryState = { state: 'degraded', service: 'gateway', reason: 'timeout' };
const REASON = 'gateway is not answering (timed out)';
const ADMIN = { scopePrn: 'prn:paigasus:iam::org/o1', roleKey: 'org.admin' };

const USERS: NavEntry = { zone: 'iam', href: '/iam/users', label: 'Users', state: AVAILABLE };
const GATEWAY: NavEntry = { zone: 'gateway', href: '/gateway/usage', label: 'Gateway', state: AVAILABLE };
const BILLING: NavEntry = { zone: 'billing', href: '/billing/invoices', label: 'Billing', state: AVAILABLE };

beforeEach(() => {
  setPathname('/');
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('PrimaryNav rules (spec § 7.2, in order)', () => {
  it('rule 1: an entry for an unconfigured zone renders nothing (AC 3, first half)', () => {
    render(inShell(<PrimaryNav entries={[USERS, BILLING]} />));
    expect(screen.getByRole('link', { name: 'Users' })).toBeInTheDocument();
    expect(screen.queryByText('Billing')).not.toBeInTheDocument();
  });

  it('rule 1 before rule 2: unconfigured AND an href that matches no zone -> nothing, no throw', () => {
    render(inShell(<PrimaryNav entries={[USERS, { ...BILLING, state: DEGRADED }]} />));
    expect(screen.queryByText('Billing')).not.toBeInTheDocument();
  });

  it('rule 1 uses the DECLARED zone, so it holds with a root zone present (spec § 6.3)', () => {
    // With a root zone, '/billing/invoices' resolves to 'console'. Without rule 1 on the declared
    // zone, rule 2 would throw here; with it, the entry is simply absent.
    render(inShell(<PrimaryNav entries={[USERS, BILLING]} />, { zones: { console: '', ...ZONES } }));
    expect(screen.queryByText('Billing')).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Users' })).toBeInTheDocument();
  });

  it('rule 2: an href that resolves to another zone than the declared one throws ZoneLinkError', () => {
    silenceReactErrorLog();
    expect(() => render(inShell(<PrimaryNav entries={[{ ...USERS, href: '/gateway/usage' }]} />))).toThrow(ZoneLinkError);
  });

  it('rule 3: absent renders nothing', () => {
    render(inShell(<PrimaryNav entries={[USERS, { ...GATEWAY, state: ABSENT }]} />));
    expect(screen.queryByText('Gateway')).not.toBeInTheDocument();
  });

  it('rule 4: `requires` with can() false renders nothing (cosmetic only, F7)', () => {
    render(inShell(<PrimaryNav entries={[USERS, { ...GATEWAY, requires: ADMIN }]} />));
    expect(screen.queryByText('Gateway')).not.toBeInTheDocument();
  });

  it('rule 4 before rule 5: can() false AND degraded -> nothing, so no outage notice for a feature the user cannot use', () => {
    render(inShell(<PrimaryNav entries={[USERS, { ...GATEWAY, state: DEGRADED, requires: ADMIN }]} />));
    expect(screen.queryByText('Gateway')).not.toBeInTheDocument();
    expect(screen.queryByText(REASON)).not.toBeInTheDocument();
  });

  it('rule 4 fails OPEN: grantsAvailable false -> the entry is shown (F7)', () => {
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, requires: ADMIN }]} />, { session: { ...SESSION, grantsAvailable: false } }));
    expect(screen.getByRole('link', { name: 'Gateway' })).toHaveAttribute('href', '/gateway/usage');
  });

  it('rule 4: a matching grant -> the entry is shown', () => {
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, requires: ADMIN }]} />, { session: { ...SESSION, grants: [ADMIN] } }));
    expect(screen.getByRole('link', { name: 'Gateway' })).toBeInTheDocument();
  });

  it('rule 5: degraded renders the disabled entry (AC 3, second half)', () => {
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, state: DEGRADED }]} />));
    expect(screen.getByRole('link', { name: 'Gateway' })).toHaveAttribute('aria-disabled', 'true');
  });

  it('rule 6: available renders a ZoneLink (a cross-zone entry is a plain <a>)', () => {
    render(inShell(<PrimaryNav entries={[USERS, GATEWAY]} />));
    expect(screen.getByRole('link', { name: 'Users' })).toHaveAttribute('data-next-link');
    expect(screen.getByRole('link', { name: 'Gateway' })).not.toHaveAttribute('data-next-link');
  });

  it('renders <nav aria-label="Primary"><ul>', () => {
    render(inShell(<PrimaryNav entries={[USERS]} />));
    const nav = screen.getByRole('navigation', { name: 'Primary' });
    expect(nav.querySelector('ul > li')).not.toBeNull();
  });

  it('every entry filtered -> no nav element at all (no empty landmark)', () => {
    const { container } = render(inShell(<PrimaryNav entries={[BILLING, { ...GATEWAY, state: ABSENT }]} />));
    expect(screen.queryByRole('navigation')).not.toBeInTheDocument();
    expect(container).toBeEmptyDOMElement();
  });
});

describe('the active entry (spec § 7.4)', () => {
  const ORGS: NavEntry = { zone: 'iam', href: '/iam/orgs', label: 'Orgs', state: AVAILABLE };
  const TEAMS: NavEntry = { zone: 'iam', href: '/iam/orgs/teams', label: 'Teams', state: AVAILABLE };
  const HOME: NavEntry = { zone: 'iam', href: '/iam', label: 'Home', state: AVAILABLE };

  it('only the LONGEST matching entry is current', () => {
    setPathname('/orgs/teams/t1');
    render(inShell(<PrimaryNav entries={[HOME, ORGS, TEAMS]} />));
    expect(screen.getByRole('link', { name: 'Teams' })).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('link', { name: 'Orgs' })).not.toHaveAttribute('aria-current');
    expect(screen.getByRole('link', { name: 'Home' })).not.toHaveAttribute('aria-current');
  });

  it('a "/" entry is current only on an exact match', () => {
    setPathname('/');
    render(inShell(<PrimaryNav entries={[HOME, ORGS]} />));
    expect(screen.getByRole('link', { name: 'Home' })).toHaveAttribute('aria-current', 'page');
  });

  it('a "/" entry is NOT current under a deeper path', () => {
    setPathname('/users');
    render(inShell(<PrimaryNav entries={[HOME, USERS]} />));
    expect(screen.getByRole('link', { name: 'Home' })).not.toHaveAttribute('aria-current');
    expect(screen.getByRole('link', { name: 'Users' })).toHaveAttribute('aria-current', 'page');
  });

  it('a cross-zone entry is never current, even when its rest path equals the pathname', () => {
    setPathname('/usage');
    render(inShell(<PrimaryNav entries={[USERS, GATEWAY]} />));
    expect(screen.getByRole('link', { name: 'Gateway' })).not.toHaveAttribute('aria-current');
  });

  it('a segment-edge near miss is not current', () => {
    setPathname('/orgsx');
    render(inShell(<PrimaryNav entries={[ORGS]} />));
    expect(screen.getByRole('link', { name: 'Orgs' })).not.toHaveAttribute('aria-current');
  });
});

describe('the degraded entry (spec § 7.3)', () => {
  it('has no <a> and no href anywhere in its list item', () => {
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, state: DEGRADED }]} />));
    const item = screen.getByRole('link', { name: 'Gateway' }).closest('li');
    expect(item).not.toBeNull();
    expect(item?.querySelector('a')).toBeNull();
    expect(item?.querySelector('[href]')).toBeNull();
  });

  it('shows the reason VISIBLY, and the reason is the accessible description', () => {
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, state: DEGRADED }]} />));
    expect(screen.getByText(REASON)).toBeVisible();
    expect(screen.getByRole('link', { name: 'Gateway' })).toHaveAccessibleDescription(REASON);
  });

  it('its accessible name is EXACTLY the label: the reason is a sibling, not a child', () => {
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, state: DEGRADED }]} />));
    const entry = screen.getByRole('link', { name: 'Gateway' });
    expect(entry).toHaveAccessibleName('Gateway');
    expect(entry).not.toContainElement(screen.getByText(REASON));
  });

  it('is focusable, so a keyboard user can reach it and hear the reason', async () => {
    const user = userEvent.setup();
    render(inShell(<PrimaryNav entries={[{ ...GATEWAY, state: DEGRADED }]} />));
    await user.tab();
    expect(screen.getByRole('link', { name: 'Gateway' })).toHaveFocus();
  });

  it('two degraded entries get distinct description ids', () => {
    render(
      inShell(
        <PrimaryNav
          entries={[
            { ...USERS, state: { state: 'degraded', service: 'iam', reason: 'network' } },
            { ...GATEWAY, state: DEGRADED },
          ]}
        />,
      ),
    );
    const ids = screen.getAllByRole('link').map((el) => el.getAttribute('aria-describedby'));
    expect(new Set(ids).size).toBe(2);
  });
});
