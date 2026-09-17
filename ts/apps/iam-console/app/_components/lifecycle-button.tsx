// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useState, type ReactElement } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from './button-class';
import type { FormAction } from './form-action';
import { FormError } from './form-error';

/**
 * The lifecycle controls of the Manage section (SMA-630 spec § 6.2). The section renders ONE of the
 * two, with `key` set to the lifecycle view, so no `useActionState` result and no `confirming` state
 * passes from one to the other. The page renders the section only for a node where mayI() allowed
 * the transition; the Server Action still asks IAM (spec § 6.3). The section's result region shows
 * the success text (spec § 6.4), so these controls show only their own error area.
 */
export type LifecycleButtonProps = {
  readonly testId: string;
  readonly prn: string;
  readonly action: FormAction;
};

/** The exact confirmation text (spec § 6.2). It names the AI Gateway effect (spec F10). */
export function archiveConfirmation(name: string): string {
  return `Archive ${name}? Until you restore it, IAM refuses changes to it and to everything under it, and the AI Gateway refuses model calls for everything under it.`;
}

/** One form, no confirmation: a restore starts traffic again, which is its purpose (spec § 10). */
export function RestoreButton({ testId, prn, action }: LifecycleButtonProps): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  return (
    <form action={formAction} aria-label="Restore this item" data-testid={testId} className="flex flex-col gap-2">
      <input type="hidden" name="prn" value={prn} />
      <button type="submit" disabled={pending} className={PRIMARY_BUTTON_CLASS}>
        Restore
      </button>
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}

/**
 * Two steps, with local state only. The first state has NO form, so with JavaScript off an archive
 * is not possible at all (spec § 6.2 accepts this). After a successful archive the page renders
 * again, and the section replaces this component with RestoreButton.
 */
export function ArchiveButton({ testId, prn, name, action }: LifecycleButtonProps & { readonly name: string }): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const [confirming, setConfirming] = useState(false);
  return (
    <div data-testid={testId} className="flex flex-col gap-2">
      {confirming ? (
        <form action={formAction} aria-label="Confirm the archive" className="flex flex-col gap-2">
          <p className="text-sm">{archiveConfirmation(name)}</p>
          <input type="hidden" name="prn" value={prn} />
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={pending} className={PRIMARY_BUTTON_CLASS}>
              Confirm archive
            </button>
            <button
              type="button"
              disabled={pending}
              className={SECONDARY_BUTTON_CLASS}
              onClick={() => {
                setConfirming(false);
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            setConfirming(true);
          }}
        >
          Archive
        </button>
      )}
      <div data-testid={`${testId}-error`}>
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </div>
  );
}
