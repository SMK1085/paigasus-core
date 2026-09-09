// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { ErrorState } from '../src/components/error-state';

describe('ErrorState', () => {
  it('renders the title as a heading', () => {
    render(<ErrorState title="Could not load keys" />);
    expect(screen.getByRole('heading', { name: 'Could not load keys' })).toBeInTheDocument();
  });

  it('renders the optional description', () => {
    render(<ErrorState title="Could not load keys" description="The server did not respond." />);
    expect(screen.getByText('The server did not respond.')).toBeInTheDocument();
  });

  it('exposes the retry control by role and calls it on activation', async () => {
    const retry = vi.fn();
    const user = userEvent.setup();
    render(<ErrorState title="Could not load keys" retry={retry} />);
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(retry).toHaveBeenCalledTimes(1);
  });

  it('has no axe violations', async () => {
    const retry = vi.fn();
    render(<ErrorState title="Could not load keys" description="The server did not respond." retry={retry} />);
    await expectNoAxeViolations();
  });
});
