// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { Breadcrumbs } from '../../src/shell/breadcrumbs';
import { ZoneLinkError } from '../../src/zone/errors';
import { expectNoAxeViolations } from '../axe';
import { silenceReactErrorLog } from '../support/console';
import { inZone } from '../support/providers';

vi.mock('next/link', () => import('../support/next-link-double'));

const TRAIL = [
  { label: 'Gateway', href: '/gateway' },
  { label: 'Users', href: '/iam/users' },
  { label: 'Ada', href: '/iam/users/u1' },
] as const;

describe('Breadcrumbs (spec § 8.4)', () => {
  it('renders <nav aria-label="Breadcrumb"><ol> with one item per crumb', () => {
    render(inZone(<Breadcrumbs items={TRAIL} />));
    const nav = screen.getByRole('navigation', { name: 'Breadcrumb' });
    expect(nav.querySelector('ol')).not.toBeNull();
    expect(screen.getAllByRole('listitem')).toHaveLength(3);
  });

  it('the last item is aria-current="page" and is NOT a link, even though it has an href', () => {
    render(inZone(<Breadcrumbs items={TRAIL} />));
    expect(screen.getByText('Ada')).toHaveAttribute('aria-current', 'page');
    expect(screen.queryByRole('link', { name: 'Ada' })).not.toBeInTheDocument();
  });

  it('earlier crumbs are ZoneLinks: a cross-zone crumb is a plain <a>, a same-zone crumb is soft', () => {
    render(inZone(<Breadcrumbs items={TRAIL} />));
    const gateway = screen.getByRole('link', { name: 'Gateway' });
    expect(gateway).not.toHaveAttribute('data-next-link');
    expect(gateway).toHaveAttribute('href', '/gateway');
    expect(screen.getByRole('link', { name: 'Users' })).toHaveAttribute('data-next-link');
  });

  it('a crumb whose href matches no zone is plain text, and the trail keeps its length', () => {
    render(inZone(<Breadcrumbs items={[{ label: 'Billing', href: '/billing' }, { label: 'Invoice' }]} />));
    expect(screen.queryByRole('link', { name: 'Billing' })).not.toBeInTheDocument();
    expect(screen.getByText('Billing')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(2);
  });

  it('a malformed href on the LAST crumb throws ZoneLinkError, even though the last crumb never links (F8)', () => {
    silenceReactErrorLog();
    expect(() =>
      render(
        inZone(
          <Breadcrumbs
            items={[
              { label: 'Users', href: '/iam/users' },
              { label: 'Ada', href: 'https://evil.example' },
            ]}
          />,
        ),
      ),
    ).toThrow(ZoneLinkError);
  });

  it('a crumb with no href before the last one is plain text', () => {
    render(inZone(<Breadcrumbs items={[{ label: 'Settings' }, { label: 'Keys' }]} />));
    expect(screen.queryByRole('link')).not.toBeInTheDocument();
    expect(screen.getByText('Settings')).not.toHaveAttribute('aria-current');
  });

  it('the separators are aria-hidden, one between each pair of crumbs', () => {
    const { container } = render(inZone(<Breadcrumbs items={TRAIL} />));
    const separators = container.querySelectorAll('[aria-hidden="true"]');
    expect(separators).toHaveLength(2);
    for (const separator of separators) expect(separator).toHaveTextContent('/');
  });

  it('an empty list renders nothing', () => {
    const { container } = render(inZone(<Breadcrumbs items={[]} />));
    expect(container).toBeEmptyDOMElement();
  });

  it('has no axe violations (AC 5)', async () => {
    render(inZone(<Breadcrumbs items={TRAIL} />));
    await expectNoAxeViolations();
  });
});
