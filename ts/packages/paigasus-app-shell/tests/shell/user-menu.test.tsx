// SPDX-License-Identifier: Apache-2.0
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { UserMenu } from '../../src/shell/user-menu';
import { SESSION, inShell } from '../support/providers';

afterEach(() => {
  vi.restoreAllMocks();
});

/** jsdom does not submit a form, so the test spies on requestSubmit (spec § 10.2). */
function spyOnRequestSubmit() {
  return vi.spyOn(HTMLFormElement.prototype, 'requestSubmit').mockImplementation(() => undefined);
}

const IDENTITIES = [
  ['displayName', { displayName: 'Ada Lovelace', email: 'ada@example.test' }, 'Ada Lovelace'],
  ['email when displayName is null', { displayName: null, email: 'ada@example.test' }, 'ada@example.test'],
  ['"Account" when both are null', { displayName: null, email: null }, 'Account'],
] as const;

describe('UserMenu (spec § 8.5)', () => {
  it.each(IDENTITIES)('the trigger shows %s, and its visible text is its accessible name', (_label, identity, text) => {
    render(inShell(<UserMenu />, { session: { ...SESSION, ...identity } }));
    const trigger = screen.getByRole('button', { name: text });
    expect(trigger).toHaveTextContent(text);
    expect(trigger).toHaveAttribute('aria-haspopup', 'menu');
  });

  it('the menu shows the name and the email, then Sign out', async () => {
    const user = userEvent.setup();
    render(inShell(<UserMenu />));
    await user.click(screen.getByRole('button', { name: 'Ada Lovelace' }));
    const menu = screen.getByRole('menu');
    expect(menu).toHaveTextContent('Ada Lovelace');
    expect(menu).toHaveTextContent('ada@example.test');
    expect(screen.getByRole('separator')).toBeInTheDocument();
    expect(screen.getByRole('menuitem', { name: 'Sign out' })).toBeInTheDocument();
    // Order matters (spec § 8.5): the label, then the separator, then "Sign out".
    const label = within(menu).getByText('Ada Lovelace');
    const separator = screen.getByRole('separator');
    const signOut = screen.getByRole('menuitem', { name: 'Sign out' });
    expect(label.compareDocumentPosition(separator) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(separator.compareDocumentPosition(signOut) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  });

  it('the logout form POSTs to the zone logout route and lives OUTSIDE the menu content', async () => {
    const user = userEvent.setup();
    const { container } = render(inShell(<UserMenu />));
    const form = container.querySelector('form');
    expect(form).not.toBeNull();
    expect(form).toHaveAttribute('method', 'post');
    expect(form).toHaveAttribute('action', '/iam/auth/logout');
    await user.click(screen.getByRole('button', { name: 'Ada Lovelace' }));
    // The portal holds the menu. The form must not be in it, and must still be mounted.
    expect(screen.getByRole('menu').contains(form)).toBe(false);
    expect(container.contains(form)).toBe(true);
  });

  it('selecting Sign out with the MOUSE calls requestSubmit on that form', async () => {
    const submit = spyOnRequestSubmit();
    const user = userEvent.setup();
    const { container } = render(inShell(<UserMenu />));
    await user.click(screen.getByRole('button', { name: 'Ada Lovelace' }));
    await user.click(screen.getByRole('menuitem', { name: 'Sign out' }));
    expect(submit).toHaveBeenCalledTimes(1);
    expect(submit.mock.contexts[0]).toBe(container.querySelector('form'));
  });

  it('selecting Sign out with the KEYBOARD calls requestSubmit on that form', async () => {
    const submit = spyOnRequestSubmit();
    const user = userEvent.setup();
    const { container } = render(inShell(<UserMenu />));
    screen.getByRole('button', { name: 'Ada Lovelace' }).focus();
    await user.keyboard('{Enter}');
    await waitFor(() => {
      expect(screen.getByRole('menuitem', { name: 'Sign out' })).toHaveFocus();
    });
    await user.keyboard('{Enter}');
    expect(submit).toHaveBeenCalledTimes(1);
    expect(submit.mock.contexts[0]).toBe(container.querySelector('form'));
  });
});
