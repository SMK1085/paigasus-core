// SPDX-License-Identifier: Apache-2.0
//
// The Manage section of the organization, team and project pages (SMA-630 spec § 5.3). A SERVER
// component: it passes each page's own three Server Actions to the client controls as props.
//
// Two inputs select the controls:
//   - mayI() (`can`). It may hide an affordance and nothing more (511 § 6.3). Every action still
//     asks IAM.
//   - D7: the lifecycle control shows ONLY the transition that the node's own status allows. An
//     active node never shows Restore, and an archived node never shows Archive. This refuses no
//     real change: a restore of an active node changes nothing (IAM accepts it), and an archive of
//     an archived node changes nothing at the repository (IAM refuses it in the default policy). The
//     'archived-parent' view shows no Restore either: a restore of the node itself does not change
//     its effective status (spec F8). No action and no command applies D7; 511 § 6.3 points here.
import type { ReactElement } from 'react';
import type { FormAction } from '../_components/form-action';
import { ArchiveButton, RestoreButton } from '../_components/lifecycle-button';
import { RenameForm } from '../_components/rename-form';
import { lifecycleView, PARENT_ARCHIVED_NOTE, type LifecycleView, type NodeLifecycle } from './node-status';

export type ManageNode = 'organization' | 'team' | 'project';

export type ManageSectionProps = {
  /** Selects the test ids (`rename-team`, …) and the copy. */
  readonly node: ManageNode;
  readonly prn: string;
  readonly name: string;
  readonly slug: string;
  readonly lifecycle: NodeLifecycle;
  readonly can: { readonly rename: boolean; readonly archive: boolean; readonly restore: boolean };
  readonly actions: { readonly rename: FormAction; readonly archive: FormAction; readonly restore: FormAction };
};

type LifecycleControl = 'archive' | 'restore' | null;

/** The "Lifecycle control" column of the spec § 5.3 table. */
function lifecycleControl(view: LifecycleView, can: ManageSectionProps['can']): LifecycleControl {
  switch (view) {
    case 'active':
    case 'archived-parent':
      return can.archive ? 'archive' : null;
    case 'archived':
      return can.restore ? 'restore' : null;
    case 'unknown':
      return null;
  }
}

export function ManageSection({ node, prn, name, slug, lifecycle, can, actions }: ManageSectionProps): ReactElement | null {
  const view = lifecycleView(lifecycle);
  const control = lifecycleControl(view, can);
  // The note shows for 'archived-parent' even with no control: with the default policy mayI() denies
  // rename and archive there (spec F9), and without the note the user sees the badge and no hint.
  const note = view === 'archived-parent';
  if (!can.rename && control === null && !note) return null;

  return (
    <section aria-labelledby="manage-heading" className="flex flex-col gap-3">
      <h2 id="manage-heading" className="text-lg font-semibold">
        Manage
      </h2>
      {note ? (
        <p data-testid="manage-note" className="text-muted-foreground text-sm">
          {PARENT_ARCHIVED_NOTE}
        </p>
      ) : null}
      {can.rename ? <RenameForm key={`${slug} ${name}`} testId={`rename-${node}`} title={`Rename ${node}`} prn={prn} slug={slug} name={name} action={actions.rename} /> : null}
      {/* `key={view}`: the two control types never share state, and a view change starts a new control. */}
      {control === 'archive' ? <ArchiveButton key={view} testId={`archive-${node}`} prn={prn} name={name} action={actions.archive} /> : null}
      {control === 'restore' ? <RestoreButton key={view} testId={`restore-${node}`} prn={prn} action={actions.restore} /> : null}
    </section>
  );
}
