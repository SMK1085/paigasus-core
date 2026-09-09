// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Dialog, DialogContent, DialogDescription, DialogTitle, DialogTrigger } from '../src/components/dialog';

function Fixture() {
  return (
    <Dialog>
      <DialogTrigger>Open</DialogTrigger>
      <DialogContent>
        <DialogTitle>Revoke key</DialogTitle>
        <DialogDescription>This cannot be undone.</DialogDescription>
      </DialogContent>
    </Dialog>
  );
}

describe('Dialog', () => {
  it('opens on trigger activation and exposes a named dialog role', async () => {
    const user = userEvent.setup();
    render(<Fixture />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open' }));
    expect(screen.getByRole('dialog', { name: 'Revoke key' })).toBeInTheDocument();
  });

  it('has no axe violations while open', async () => {
    const user = userEvent.setup();
    render(<Fixture />);
    await user.click(screen.getByRole('button', { name: 'Open' }));
    // No argument: the content is portalled onto document.body, so a container-scoped
    // assertion would check a subtree holding only the trigger.
    await expectNoAxeViolations();
  });
});
