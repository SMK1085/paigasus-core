// SPDX-License-Identifier: Apache-2.0
// @vitest-environment jsdom
//
// The two-step confirm control (SMA-636 spec § 5.5, § 5.6): a copy of iam-console's ArchiveButton.
// The first state has NO form and NO submit, so with JavaScript off the action cannot run.
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ConfirmButton, type ConfirmButtonProps } from '../../app/_components/confirm-button';

afterEach(() => {
  cleanup();
});

function renderButton(overrides: Partial<ConfirmButtonProps> = {}) {
  const onConfirm = vi.fn<(form: FormData) => void>();
  render(
    <ConfirmButton
      testId="revoke-key-1"
      label="Revoke"
      confirmLabel="Confirm revoke"
      confirmation="Revoke the key pgs_a?"
      hidden={{ saPrn: 'sa-1', keyId: 'key-1' }}
      disabled={false}
      onConfirm={onConfirm}
      {...overrides}
    />,
  );
  return onConfirm;
}

describe('ConfirmButton', () => {
  it('has no form and no submit before the first click', () => {
    renderButton();
    expect(screen.queryByRole('form')).toBeNull();
    expect(screen.getByRole('button', { name: 'Revoke' }).getAttribute('type')).toBe('button');
  });

  it('asks first, then hands the hidden fields to onConfirm', async () => {
    const user = userEvent.setup();
    const onConfirm = renderButton();

    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    expect(screen.getByText('Revoke the key pgs_a?')).toBeDefined();
    await user.click(screen.getByRole('button', { name: 'Confirm revoke' }));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const form = onConfirm.mock.calls[0]?.[0];
    expect(form?.get('saPrn')).toBe('sa-1');
    expect(form?.get('keyId')).toBe('key-1');
  });

  it('goes back without a call on Cancel', async () => {
    const user = userEvent.setup();
    const onConfirm = renderButton();

    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));

    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.queryByRole('form')).toBeNull();
  });

  it('disables the first step while disabled', () => {
    renderButton({ disabled: true });
    expect(screen.getByRole<HTMLButtonElement>('button', { name: 'Revoke' }).disabled).toBe(true);
  });
});
