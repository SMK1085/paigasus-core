// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Link } from '../src/nav/link';
import { EmptyState } from '../src/components/empty-state';

describe('EmptyState', () => {
  it('renders the title as a heading', () => {
    render(<EmptyState title="No API keys yet" />);
    expect(screen.getByRole('heading', { name: 'No API keys yet' })).toBeInTheDocument();
  });

  it('renders the optional description', () => {
    render(<EmptyState title="No API keys yet" description="Create one to get started." />);
    expect(screen.getByText('Create one to get started.')).toBeInTheDocument();
  });

  it('exposes the action slot by role', () => {
    render(<EmptyState title="No API keys yet" action={<Link href="/keys/new">Create key</Link>} />);
    expect(screen.getByRole('link', { name: 'Create key' })).toBeInTheDocument();
  });

  it('has no axe violations', async () => {
    render(<EmptyState title="No API keys yet" description="Create one to get started." action={<Link href="/keys/new">Create key</Link>} />);
    await expectNoAxeViolations();
  });
});
