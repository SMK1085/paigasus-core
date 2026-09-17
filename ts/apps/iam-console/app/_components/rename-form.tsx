// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useId, useState, type ReactElement } from 'react';
import { Field, Input } from '@paigasus/ui';
import { PRIMARY_BUTTON_CLASS } from './button-class';
import type { FormAction } from './form-action';
import { FormError } from './form-error';

/**
 * The rename form of a tenancy node (SMA-630 spec § 6.1). The hidden `currentSlug` and
 * `currentName` let the command send only the fields that the user changed (D6).
 *
 * THE INPUTS ARE CONTROLLED. React 19 resets a form before it runs every action, whatever the result
 * (react-dom-client.development.js:8954-8957). Controlled inputs keep their React state through that
 * reset, so after a failed rename the typed values stay next to the error. The Manage section
 * renders this component with `key={`${slug} ${name}`}`: after a successful rename the page renders
 * again with new props, the key changes, and the form starts again from the new values, with a new
 * `useActionState`. The section's result region shows "Renamed." (spec § 6.4), so this form shows
 * no success text.
 *
 * WHILE THE ACTION RUNS, the inputs are read-only, so a failure shows next to the values that were
 * sent. They are not `disabled`: the form data leaves out a disabled input.
 */
export type RenameFormProps = {
  readonly testId: string;
  readonly title: string;
  readonly prn: string;
  readonly slug: string;
  readonly name: string;
  readonly action: FormAction;
};

export function RenameForm({ testId, title, prn, slug: currentSlug, name: currentName, action }: RenameFormProps): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const [slug, setSlug] = useState(currentSlug);
  const [name, setName] = useState(currentName);
  const id = useId();
  return (
    <form action={formAction} aria-label={title} data-testid={testId} className="flex max-w-md flex-col gap-3">
      <h3 className="text-sm font-medium">{title}</h3>
      <input type="hidden" name="prn" value={prn} />
      <input type="hidden" name="currentSlug" value={currentSlug} />
      <input type="hidden" name="currentName" value={currentName} />
      <Field label="Slug" htmlFor={`${id}-slug`}>
        <Input
          name="slug"
          required
          autoComplete="off"
          readOnly={pending}
          value={slug}
          onChange={(event) => {
            setSlug(event.target.value);
          }}
        />
      </Field>
      <Field label="Name" htmlFor={`${id}-name`}>
        <Input
          name="name"
          required
          autoComplete="off"
          readOnly={pending}
          value={name}
          onChange={(event) => {
            setName(event.target.value);
          }}
        />
      </Field>
      <button type="submit" disabled={pending} className={PRIMARY_BUTTON_CLASS}>
        Rename
      </button>
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}
