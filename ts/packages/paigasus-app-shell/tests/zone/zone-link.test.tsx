// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef, type MouseEvent } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Link, LinkProvider } from '@paigasus/ui';
import { ZoneLinkError } from '../../src/zone/errors';
import { ZoneLink } from '../../src/zone/zone-link';
import { silenceReactErrorLog } from '../support/console';
import { nextLinkClicks } from '../support/next-link-double';
import { inZone } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));

beforeEach(() => {
  nextLinkClicks.length = 0;
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
});

describe('ZoneLink (spec § 6.4)', () => {
  it('same zone: the next/link double, with the zone-relative remainder (Next adds the base path back, F3)', () => {
    render(inZone(<ZoneLink href="/iam/users?page=2#top">Users</ZoneLink>));
    const link = screen.getByRole('link', { name: 'Users' });
    expect(link).toHaveAttribute('data-next-link');
    expect(link).toHaveAttribute('href', '/users?page=2#top');
  });

  it('same zone, the base path itself: the remainder is "/"', () => {
    render(inZone(<ZoneLink href="/iam">Home</ZoneLink>));
    expect(screen.getByRole('link', { name: 'Home' })).toHaveAttribute('href', '/');
  });

  it('same zone under /auth/: a plain <a> with the full href, never next/link (spec § 6.5)', () => {
    render(inZone(<ZoneLink href="/iam/auth/login">Sign in</ZoneLink>));
    const link = screen.getByRole('link', { name: 'Sign in' });
    expect(link).not.toHaveAttribute('data-next-link');
    expect(link).toHaveAttribute('href', '/iam/auth/login');
  });

  it('cross zone: a plain <a> with the href unchanged (AC 1)', () => {
    render(inZone(<ZoneLink href="/gateway/usage">Usage</ZoneLink>));
    const link = screen.getByRole('link', { name: 'Usage' });
    expect(link).not.toHaveAttribute('data-next-link');
    expect(link).toHaveAttribute('href', '/gateway/usage');
  });

  it('no match: renders nothing, and warns ONCE per first segment without the full href', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { container, rerender } = render(inZone(<ZoneLink href="/orphans?token=secret-abc">Orphans</ZoneLink>));
    expect(container).toBeEmptyDOMElement();
    rerender(inZone(<ZoneLink href="/orphans/other">Orphans</ZoneLink>));
    expect(warn).toHaveBeenCalledTimes(1);
    const message = String(warn.mock.calls[0]?.[0]);
    expect(message).toContain('/orphans');
    expect(message).not.toContain('secret-abc');
  });

  it('no match in a production build: renders nothing and does not warn', () => {
    vi.stubEnv('NODE_ENV', 'production');
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const { container } = render(inZone(<ZoneLink href="/quiet-segment">Quiet</ZoneLink>));
    expect(container).toBeEmptyDOMElement();
    expect(warn).not.toHaveBeenCalled();
  });

  it('a malformed href throws ZoneLinkError instead of falling back to a plain <a>', () => {
    silenceReactErrorLog();
    expect(() => render(inZone(<ZoneLink href="https://evil.example/">x</ZoneLink>))).toThrow(ZoneLinkError);
  });

  it('forwards onClick on the same-zone next/link branch (F1)', async () => {
    const onClick = vi.fn();
    render(
      inZone(
        <ZoneLink href="/iam/users" onClick={onClick}>
          Users
        </ZoneLink>,
      ),
    );
    await userEvent.click(screen.getByRole('link', { name: 'Users' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('a same-zone onClick that calls preventDefault() records no click (double parity with next/link)', async () => {
    const onClick = vi.fn((event: MouseEvent<HTMLAnchorElement>) => {
      event.preventDefault();
    });
    render(
      inZone(
        <ZoneLink href="/iam/users" onClick={onClick}>
          Users
        </ZoneLink>,
      ),
    );
    await userEvent.click(screen.getByRole('link', { name: 'Users' }));
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(nextLinkClicks).toEqual([]);
  });

  it('forwards onClick on the cross-zone plain <a> branch (F1)', async () => {
    const onClick = vi.fn();
    render(
      inZone(
        <ZoneLink href="/gateway/usage" onClick={onClick}>
          Usage
        </ZoneLink>,
      ),
    );
    await userEvent.click(screen.getByRole('link', { name: 'Usage' }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('forwards ref, aria-* and data-* on both element kinds (what Radix asChild passes)', () => {
    const same = createRef<HTMLAnchorElement>();
    const cross = createRef<HTMLAnchorElement>();
    render(
      inZone(
        <>
          <ZoneLink ref={same} href="/iam/users" aria-label="Same zone">
            a
          </ZoneLink>
          <ZoneLink ref={cross} href="/gateway/usage" aria-describedby="hint" data-probe="x" tabIndex={-1}>
            b
          </ZoneLink>
          <span id="hint">hint</span>
        </>,
      ),
    );
    expect(same.current).toBe(screen.getByRole('link', { name: 'Same zone' }));
    expect(cross.current).toHaveAttribute('aria-describedby', 'hint');
    expect(cross.current).toHaveAttribute('data-probe', 'x');
    expect(cross.current).toHaveAttribute('tabindex', '-1');
  });
});

describe('@paigasus/ui injection: <LinkProvider link={ZoneLink}> (spec § 6.4, § 10.2)', () => {
  it('a ui Link to ANOTHER zone renders a plain <a>', () => {
    render(
      inZone(
        <LinkProvider link={ZoneLink}>
          <Link href="/gateway/ui">Gateway</Link>
        </LinkProvider>,
      ),
    );
    const link = screen.getByRole('link', { name: 'Gateway' });
    expect(link).not.toHaveAttribute('data-next-link');
    expect(link).toHaveAttribute('href', '/gateway/ui');
  });

  it('a ui Link to the SAME zone renders the next/link double — the row that fails with no injection', () => {
    // The @paigasus/ui context default is already a plain <a> (F4), so the row above passes with NO
    // injection at all. This row is the one that proves LinkProvider took effect.
    render(
      inZone(
        <LinkProvider link={ZoneLink}>
          <Link href="/iam/settings">Settings</Link>
        </LinkProvider>,
      ),
    );
    const link = screen.getByRole('link', { name: 'Settings' });
    expect(link).toHaveAttribute('data-next-link');
    expect(link).toHaveAttribute('href', '/settings');
  });
});
