// SPDX-License-Identifier: Apache-2.0
'use client';

import { useRef, useState, type ReactElement } from 'react';
import type { ActionState, FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
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
 * unmounts with its `useActionState` result. The section renders this component keyed by the node
 * PRN (`key={prn}`): the key is stable during a refresh of the SAME node, so React keeps its state,
 * and the result, through the refresh. A move to a DIFFERENT node changes the key, so React starts a
 * new instance with an empty region.
 *
 * THE RULE.
 *   - A success shows ONLY here: "Renamed.", "Archived." or "Restored.". The controls show no
 *     success text.
 *   - A failure shows in the error area of the control that produced it, while that control is
 *     mounted. The region shows the failure only after that control has unmounted (its key changed
 *     or it is not rendered). So the same error never shows twice.
 *   - A new submission replaces the previous result. It also clears the error of every OTHER
 *     control: each control has a reset counter in its key, and a submission increments the counters
 *     of the other controls. The control that submits keeps its key, so a failed rename keeps its
 *     typed values (§ 6.1).
 *   - Only the last submission sets the result. Each submission records a generation number before
 *     it waits for the action. When an older submission completes after a newer one started, this
 *     component ignores its result. The wrapped action still returns the result to its control.
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
  const [resets, setResets] = useState<Record<ManageControl, number>>({ rename: 0, archive: 0, restore: 0 });
  const generationRef = useRef(0);
  // A space is a safe separator: the counter has only digits, and IAM allows only [a-z0-9-] in a
  // slug (spec § 6.1).
  const renameKey = `${resets.rename} ${slug} ${name}`;
  // The key of each control that is mounted now, or null. The same values are the `key` props below.
  const mounted: Record<ManageControl, string | null> = {
    rename: rename ? renameKey : null,
    archive: lifecycle === 'archive' ? `${resets.archive} ${view}` : null,
    restore: lifecycle === 'restore' ? `${resets.restore} ${view}` : null,
  };

  // `mount` is the key of the control instance that runs the action. A remount makes a new instance,
  // and its useActionState uses the action from its own renders, so the captured key is its own key.
  function recording(control: ManageControl, mount: string, action: FormAction): FormAction {
    return async (previous, form) => {
      generationRef.current += 1;
      const current = generationRef.current;
      setResult(null);
      // Remount the OTHER controls, so that their previous errors go. The control that submits keeps its key.
      setResets((counts) => ({
        rename: control === 'rename' ? counts.rename : counts.rename + 1,
        archive: control === 'archive' ? counts.archive : counts.archive + 1,
        restore: control === 'restore' ? counts.restore : counts.restore + 1,
      }));
      const state: ActionState = await action(previous, form);
      // A later submission started while this one waited: its result replaces this one.
      if (generationRef.current === current) {
        setResult(state === null ? null : state.ok ? { ok: true, control } : { ok: false, control, mount, error: state.error });
      }
      return state;
    };
  }

  const failure = result?.ok === false && mounted[result.control] !== result.mount ? result.error : null;

  return (
    <>
      {rename ? <RenameForm key={renameKey} testId={`rename-${node}`} title={`Rename ${node}`} prn={prn} slug={slug} name={name} action={recording('rename', renameKey, actions.rename)} /> : null}
      {/* The key holds the view: the two control types never share state, and a view change starts a new control. */}
      {mounted.archive !== null ? <ArchiveButton key={mounted.archive} testId={`archive-${node}`} prn={prn} name={name} action={recording('archive', mounted.archive, actions.archive)} /> : null}
      {mounted.restore !== null ? <RestoreButton key={mounted.restore} testId={`restore-${node}`} prn={prn} action={recording('restore', mounted.restore, actions.restore)} /> : null}
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
