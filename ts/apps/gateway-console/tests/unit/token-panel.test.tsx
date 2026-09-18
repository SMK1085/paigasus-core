// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// TokenPanel (SMA-636 spec § 5.4 rules 3 and 6): the token lives in this component's own state. It
// closes on "Done" and on `pagehide`, and on nothing else.
import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef } from 'react';
import { afterEach, describe, expect, it } from 'vitest';
import { TokenPanel, type TokenPanelHandle } from '../../app/_components/token-panel';

const TOKEN = 'pgs_unit_0123456789abcdef0123456789abcdef';
const SECOND = 'pgs_unit_fedcba9876543210fedcba9876543210';

afterEach(() => {
  cleanup();
});

function mount() {
  const ref = createRef<TokenPanelHandle>();
  render(<TokenPanel ref={ref} />);
  return ref;
}

describe('TokenPanel', () => {
  it('shows nothing until a key is issued', () => {
    mount();
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('shows the token, its prefix and the one-time warning', () => {
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    expect(screen.getByTestId('token-value').textContent).toBe(TOKEN);
    expect(screen.getByText('You cannot see this token again.')).toBeDefined();
    expect(screen.getByTestId('token-panel').textContent).toContain('pgs_unit_01');
  });

  it('closes on Done', async () => {
    const user = userEvent.setup();
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    await user.click(screen.getByRole('button', { name: 'Done' }));
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('closes on pagehide', () => {
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    act(() => {
      window.dispatchEvent(new Event('pagehide'));
    });
    expect(screen.queryByTestId('token-panel')).toBeNull();
  });

  it('shows the second token when a second key is issued', () => {
    const ref = mount();
    act(() => {
      ref.current?.show(TOKEN, 'pgs_unit_01');
    });
    act(() => {
      ref.current?.show(SECOND, 'pgs_unit_fe');
    });
    expect(screen.getByTestId('token-value').textContent).toBe(SECOND);
  });
});
