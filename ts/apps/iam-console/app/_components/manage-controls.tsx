// SPDX-License-Identifier: Apache-2.0
'use client';

import { useState, type ReactElement } from 'react';
import type { ActionState } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import type { FormAction } from './form-action';
import { FormError } from './form-error';
import { ArchiveButton, RestoreButton } from './lifecycle-button';
import { RenameForm } from './rename-form';

/**
 * The controls of the Manage section and its result region (SMA-630 spec § 6.4). The server
 * section decides WHICH controls show (§ 5.3); this component renders them and keeps the last
 * result.
 *
 * WHY THE REGION EXISTS. A rename, archive or restore action refreshes the page on a success and
 * on `forbidden`/`conflict` (§ 4.4). The action result and the refreshed page commit together. The
 * refresh can change the lifecycle view or the rename key, and then the control that ran the action
 * unmounts with its `useActionState` result. The section renders this component with NO key at a
 * stable position, so React keeps its state through the refresh.
 *
 * THE RULE.
 *   - A success shows ONLY here: "Renamed.", "Archived." or "Restored.". The controls show no
 *     success text.
 *   - A failure shows in the error area of the control that produced it, while that control is
 *     mounted. The region shows the failure only after that control has unmounted (its key changed
 *     or it is not rendered). So the same error never shows twice.
 *   - A new submission replaces the previous result.
 */
export type ManageControl = 'rename' | 'archive' | 'restore';

export type ManageControlsProps = {
  readonly node: string;
  readonly prn: string;
  readonly name: string;
  readonly slug: string;
  /** The key of the lifecycle control: the lifecycle view. */
  readonly view: string;
  readonly rename: boolean;
  readonly lifecycle: 'archive' | 'restore' | null;
  readonly actions: { readonly rename: FormAction; readonly archive: FormAction; readonly restore: FormAction };
};

type ManageResult = { readonly ok: true; readonly control: ManageControl } | { readonly ok: false; readonly control: ManageControl; readonly mount: string; readonly error: PaigasusError } | null;

const SUCCESS_TEXT: Record<ManageControl, string> = { rename: 'Renamed.', archive: 'Archived.', restore: 'Restored.' };

export function ManageControls({ node, prn, name, slug, view, rename, lifecycle, actions }: ManageControlsProps): ReactElement {
  const [result, setResult] = useState<ManageResult>(null);
  // A space is a safe separator: IAM allows only [a-z0-9-] in a slug (spec § 6.1).
  const renameKey = `${slug} ${name}`;
  // The key of each control that is mounted now, or null. The same values are the `key` props below.
  const mounted: Record<ManageControl, string | null> = {
    rename: rename ? renameKey : null,
    archive: lifecycle === 'archive' ? view : null,
    restore: lifecycle === 'restore' ? view : null,
  };

  // `mount` is the key of the control instance that runs the action. A remount makes a new instance,
  // and its useActionState uses the action from its own renders, so the captured key is its own key.
  function recording(control: ManageControl, mount: string, action: FormAction): FormAction {
    return async (previous, form) => {
      setResult(null);
      const state: ActionState = await action(previous, form);
      setResult(state === null ? null : state.ok ? { ok: true, control } : { ok: false, control, mount, error: state.error });
      return state;
    };
  }

  const failure = result?.ok === false && mounted[result.control] !== result.mount ? result.error : null;

  return (
    <>
      {rename ? <RenameForm key={renameKey} testId={`rename-${node}`} title={`Rename ${node}`} prn={prn} slug={slug} name={name} action={recording('rename', renameKey, actions.rename)} /> : null}
      {/* `key={view}`: the two control types never share state, and a view change starts a new control. */}
      {lifecycle === 'archive' ? <ArchiveButton key={view} testId={`archive-${node}`} prn={prn} name={name} action={recording('archive', view, actions.archive)} /> : null}
      {lifecycle === 'restore' ? <RestoreButton key={view} testId={`restore-${node}`} prn={prn} action={recording('restore', view, actions.restore)} /> : null}
      <div data-testid="manage-result">
        {result?.ok === true ? (
          <p role="status" className="text-sm">
            {SUCCESS_TEXT[result.control]}
          </p>
        ) : null}
        <FormError error={failure} />
      </div>
    </>
  );
}
