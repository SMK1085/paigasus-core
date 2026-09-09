// SPDX-License-Identifier: Apache-2.0
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { expectNoAxeViolations } from './axe';
import { Field, Input } from '../src/components/form';

describe('Field', () => {
  it('associates the label with the control', () => {
    render(
      <Field label="Key name" htmlFor="key-name">
        <Input id="key-name" />
      </Field>,
    );
    expect(screen.getByLabelText('Key name')).toBeInTheDocument();
  });

  it('exposes the error to assistive technology', () => {
    render(
      <Field label="Key name" htmlFor="key-name" error="Name is required.">
        <Input id="key-name" />
      </Field>,
    );
    const input = screen.getByLabelText('Key name');
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(input).toHaveAccessibleDescription(/Name is required\./);
  });

  it('has no axe violations', async () => {
    render(
      <Field label="Key name" htmlFor="key-name" description="Shown in the audit log.">
        <Input id="key-name" />
      </Field>,
    );
    await expectNoAxeViolations();
  });

  it('preserves a caller-supplied aria-describedby alongside the generated ids', () => {
    render(
      <Field label="Key name" htmlFor="key-name" description="Shown in the audit log." error="Name is required.">
        <Input id="key-name" aria-describedby="external-hint" />
      </Field>,
    );
    const input = screen.getByLabelText('Key name');
    const describedBy = input.getAttribute('aria-describedby')?.split(' ') ?? [];
    expect(describedBy).toContain('external-hint');
    expect(describedBy).toContain('key-name-description');
    expect(describedBy).toContain('key-name-error');
  });

  it('throws when there is no element child to wire', () => {
    expect(() =>
      render(
        <Field label="Key name" htmlFor="key-name">
          {null}
        </Field>,
      ),
    ).toThrow(/requires exactly one element child/);
  });

  it('throws when children is an array instead of a single element', () => {
    expect(() =>
      render(
        <Field label="Key name" htmlFor="key-name">
          {[<Input key="control" id="key-name" />, <span key="hint">Required</span>]}
        </Field>,
      ),
    ).toThrow(/requires exactly one element child/);
  });

  it('throws when children is a raw string', () => {
    expect(() =>
      render(
        <Field label="Key name" htmlFor="key-name">
          {'just text'}
        </Field>,
      ),
    ).toThrow(/requires exactly one element child/);
  });

  /*
   * SMA-503 local review, finding F. A Fragment passes isValidElement, so the guard above let
   * it through and cloneElement dropped id, aria-describedby and aria-invalid in silence — an
   * unwired control that renders looking exactly like a wired one. The message must name a
   * fragment, not "an unsupported child", or the caller has nothing to act on.
   */
  it('throws when children is a fragment', () => {
    expect(() =>
      render(
        <Field label="Key name" htmlFor="key-name">
          <>
            <Input />
          </>
        </Field>,
      ),
    ).toThrow(/received a fragment/);
  });

  /*
   * SMA-503 fix round 2, item 2. Before the id injection the caller had to write the same
   * string twice, and a typo or a rename broke the label association with no error and no
   * failing test. This test renders NO id at all on the control.
   */
  it('injects htmlFor as the control id when the child carries none', () => {
    render(
      <Field label="Key name" htmlFor="key-name">
        <Input />
      </Field>,
    );
    const input = screen.getByLabelText('Key name');
    expect(input).toHaveAttribute('id', 'key-name');
  });

  it('throws when the control carries an id that disagrees with htmlFor', () => {
    expect(() =>
      render(
        <Field label="Key name" htmlFor="key-name">
          <Input id="typo-name" />
        </Field>,
      ),
    ).toThrow(/does not match its control's own id/);
  });
});
