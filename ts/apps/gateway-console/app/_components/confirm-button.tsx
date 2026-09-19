// SPDX-License-Identifier: Apache-2.0
//
// A two-step confirm control (SMA-636 spec § 4.6, § 5.5, § 5.6). COPY of iam-console's
// ArchiveButton (app/_components/lifecycle-button.tsx), recorded in spec § 9; nothing gates a
// divergence. Two changes: it takes its words and its hidden fields as props, and it hands the
// submitted form to `onConfirm` instead of posting it, because the section runs every action in
// its own transition and shows every result in ONE result region (§ 4.6). So it has no error area.
//
// The first state has NO form, so with JavaScript off the action is not possible at all.
'use client';

import { useState, type ReactElement } from 'react';
import { PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS } from '@paigasus/ui';

export type ConfirmButtonProps = {
  readonly testId: string;
  readonly label: string;
  readonly confirmLabel: string;
  readonly confirmation: string;
  readonly hidden: Readonly<Record<string, string>>;
  readonly disabled: boolean;
  readonly onConfirm: (form: FormData) => void;
};

export function ConfirmButton({ testId, label, confirmLabel, confirmation, hidden, disabled, onConfirm }: ConfirmButtonProps): ReactElement {
  const [confirming, setConfirming] = useState(false);
  return (
    <div data-testid={testId} className="flex flex-col gap-2">
      {confirming ? (
        <form
          aria-label={confirmLabel}
          className="flex flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            onConfirm(new FormData(event.currentTarget));
            setConfirming(false);
          }}
        >
          <p className="text-sm">{confirmation}</p>
          {Object.entries(hidden).map(([name, value]) => (
            <input key={name} type="hidden" name={name} value={value} />
          ))}
          <div className="flex flex-wrap gap-2">
            <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
              {confirmLabel}
            </button>
            <button
              type="button"
              disabled={disabled}
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
          disabled={disabled}
          className={SECONDARY_BUTTON_CLASS}
          onClick={() => {
            setConfirming(true);
          }}
        >
          {label}
        </button>
      )}
    </div>
  );
}
