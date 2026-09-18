// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section body (SMA-636 spec § 4.6): the read-only note, the create form, the
// list, the pager and the selected-account panel. CLIENT component. It renders inside
// ServiceAccountFrame (./service-account-frame.tsx), which owns the ONE result region, the
// TokenPanel and the runners that call the actions. The frame survives a revalidated render that
// turns this body into an error or a denial, so a shown token stays (§ 5.4 rule 3).
// Every submit control renders only after hydration (rule 7; plan SPEC DEVIATION 7 for the rest).
'use client';

import { useId, type ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { EmptyState, Field, Input, PRIMARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { linkHref } from '../../lib/paging';
import { PARENT_ARCHIVED_NOTE } from '../(console)/node-status';
import type { OwnerKind, ReadOnlyView, SectionOk, ServiceAccountActions, ServiceAccountRowView } from '../(console)/service-accounts/view';
import { Pager } from './pager';
import { useSectionRunner } from './service-account-frame';
import { ServiceAccountPanel } from './service-account-panel';
import { useHydrated } from './use-hydrated';

export type ServiceAccountSectionProps = {
  readonly ownerKind: OwnerKind;
  /** The page's full path (/gateway/orgs/<org>[/projects/<project>]), for the select and pager links. */
  readonly path: string;
  readonly view: SectionOk;
  readonly actions: ServiceAccountActions;
};

/** § 5.8. `unknown` is read-only with no note. */
function readOnlyNote(readOnly: ReadOnlyView, ownerKind: OwnerKind): string | null {
  if (readOnly === 'archived') return `This ${ownerKind} is archived. IAM refuses changes to its service accounts and keys.`;
  if (readOnly === 'archived-parent') return PARENT_ARCHIVED_NOTE;
  return null;
}

export function CreateServiceAccountForm({ ownerPrn, disabled, onSubmit }: { readonly ownerPrn: string; readonly disabled: boolean; readonly onSubmit: (form: FormData) => void }): ReactElement {
  const id = useId();
  return (
    <form
      aria-label="Create service account"
      data-testid="sa-create-form"
      className="flex max-w-md flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        onSubmit(new FormData(event.currentTarget));
      }}
    >
      <h3 className="text-sm font-medium">Create service account</h3>
      {/* Under client control, and safe: IAM checks CreateServiceAccount at this node (§ 5.1). */}
      <input type="hidden" name="ownerPrn" value={ownerPrn} />
      <Field label="Name" htmlFor={`${id}-name`}>
        <Input name="name" required autoComplete="off" />
      </Field>
      <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
        Create
      </button>
    </form>
  );
}

export function ServiceAccountRow({
  row,
  path,
  saOffset,
  selected,
}: {
  readonly row: ServiceAccountRowView;
  readonly path: string;
  readonly saOffset: number;
  readonly selected: boolean;
}): ReactElement {
  return (
    <TableRow data-testid="sa-row" data-selected={String(selected)}>
      <TableCell>{row.name}</TableCell>
      <TableCell>{row.created ?? ''}</TableCell>
      <TableCell>
        {row.active ? (
          'Active'
        ) : (
          <span data-testid="sa-row-archived" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
            Archived
          </span>
        )}
      </TableCell>
      <TableCell>
        {row.id === null ? null : (
          <ZoneLink prefetch={false} href={linkHref(path, { saOffset, sa: row.id })} aria-label={`Select ${row.name}`} className="hover:underline">
            Select
          </ZoneLink>
        )}
      </TableCell>
    </TableRow>
  );
}

function ServiceAccountList({ view, path }: { readonly view: SectionOk; readonly path: string }): ReactElement {
  if (view.rows.length === 0 && view.saOffset === 0) return <EmptyState title="No service accounts yet" />;
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Created</TableHead>
          <TableHead>Status</TableHead>
          <TableHead>
            <span className="sr-only">Select</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {view.rows.map((row) => (
          <ServiceAccountRow key={row.prn} row={row} path={path} saOffset={view.saOffset} selected={row.id !== null && row.id === view.sa} />
        ))}
      </TableBody>
    </Table>
  );
}

export function ServiceAccountSection({ ownerKind, path, view, actions }: ServiceAccountSectionProps): ReactElement {
  const hydrated = useHydrated();
  const runner = useSectionRunner();
  const busy = runner.busy;
  const note = readOnlyNote(view.readOnly, ownerKind);

  return (
    <div className="flex flex-col gap-4">
      {note === null ? null : (
        <p data-testid="sa-read-only" className="text-muted-foreground text-sm">
          {note}
        </p>
      )}
      {view.canCreate && hydrated ? (
        <CreateServiceAccountForm
          ownerPrn={view.ownerPrn}
          disabled={busy}
          onSubmit={(form) => {
            runner.runCreate(actions.create, form);
          }}
        />
      ) : null}
      <ServiceAccountList view={view} path={path} />
      <Pager label="Service account pages" path={path} param="saOffset" offset={view.page.offset} nextOffset={view.page.nextOffset} keep={{ sa: view.sa }} />
      <ServiceAccountPanel
        selected={view.selected}
        path={path}
        saOffset={view.saOffset}
        sa={view.sa}
        hydrated={hydrated}
        disabled={busy}
        onAllow={(form) => {
          runner.run('allow', actions.allow, form);
        }}
        onRevoke={(form) => {
          runner.run('revoke', actions.revoke, form);
        }}
        onArchive={(form) => {
          runner.run('archive', actions.archive, form);
        }}
        onIssue={(form) => {
          runner.runIssue(actions.issue, form);
        }}
      />
    </div>
  );
}
