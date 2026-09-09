// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Link, LinkProvider } from '../src/nav/link';

function StubLink({ href, className, children }: { href: string; className?: string; children?: ReactNode }): ReactElement {
  return (
    <a data-testid="stub" href={href} className={className}>
      {children}
    </a>
  );
}

describe('Link', () => {
  it('renders a plain anchor with no provider, so the package needs no framework', () => {
    render(<Link href="/iam">IAM</Link>);
    const anchor = screen.getByRole('link', { name: 'IAM' });
    expect(anchor).toHaveAttribute('href', '/iam');
    expect(anchor).not.toHaveAttribute('data-testid');
  });

  it('uses the injected component when a provider supplies one', () => {
    render(
      <LinkProvider link={StubLink}>
        <Link href="/gateway">Gateway</Link>
      </LinkProvider>,
    );
    expect(screen.getByTestId('stub')).toHaveAttribute('href', '/gateway');
  });

  it('passes className through to the injected component', () => {
    render(
      <LinkProvider link={StubLink}>
        <Link href="/gateway" className="text-primary">
          Gateway
        </Link>
      </LinkProvider>,
    );
    expect(screen.getByTestId('stub')).toHaveClass('text-primary');
  });

  it('has no axe violations', async () => {
    render(<Link href="/iam">IAM</Link>);
    await expectNoAxeViolations();
  });
});
