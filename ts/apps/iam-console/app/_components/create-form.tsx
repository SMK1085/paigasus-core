// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useId, type ReactElement } from 'react';
import { Field, Input } from '@paigasus/ui';
// A TYPE import: verbatimModuleSyntax erases it, so no server-only module reaches the client bundle.
import type { ActionState } from '../../lib/errors';
import { FormError } from './form-error';

/**
 * The one "create" form of the tenancy screens: a slug, a name and a submit button (spec § 5.3).
 * A page renders it only when mayI() allowed it. The Server Action it posts to never asks mayI():
 * IAM decides (spec § 6.3). `hidden` carries the parent PRN.
 */
export type CreateFormProps = {
  readonly testId: string;
  readonly title: string;
  readonly submitLabel: string;
  readonly action: (previous: ActionState, form: FormData) => Promise<ActionState>;
  readonly hidden?: Readonly<Record<string, string>>;
};

export function CreateForm({ testId, title, submitLabel, action, hidden = {} }: CreateFormProps): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const id = useId();
  return (
    <form action={formAction} aria-label={title} data-testid={testId} className="flex max-w-md flex-col gap-3">
      <h3 className="text-sm font-medium">{title}</h3>
      {Object.entries(hidden).map(([name, value]) => (
        <input key={name} type="hidden" name={name} value={value} />
      ))}
      <Field label="Slug" htmlFor={`${id}-slug`}>
        <Input name="slug" required autoComplete="off" />
      </Field>
      <Field label="Name" htmlFor={`${id}-name`}>
        <Input name="name" required autoComplete="off" />
      </Field>
      <button type="submit" disabled={pending} className="bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50">
        {submitLabel}
      </button>
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Created.
        </p>
      ) : null}
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}
