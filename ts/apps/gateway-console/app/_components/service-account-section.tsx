// SPDX-License-Identifier: Apache-2.0
//
// The service-accounts section (SMA-636 spec § 4.6): the frame, ONE result region for every action
// of the section, the create form, the list, the pager and the selected-account panel. CLIENT
// component. The server block renders it with `key={ownerPrn}`, so its state — the result region
// and the token panel — stays through a revalidation of the SAME owner, and a move to another owner
// starts a new, empty instance (the ManageControls pattern, SMA-630).
//
// WHY ONE REGION. A row or panel control that succeeds often unmounts itself: a revoked key loses
// its Revoke button, an archived account loses every control. So every result, success or error,
// goes to this region, and no control holds the only copy of its result.
//
// HOW AN ACTION RUNS. Every control hands its form to this component, which calls the action
// DIRECTLY in a transition, with `null` as the previous state. § 5.4 rule 2 requires that for the
// issue action, because useActionState sends the previous state — the token — back to the server;
// the other actions follow the same path. A newer submission replaces the result of an older one
// (a generation counter), EXCEPT an issue result: a token that IAM minted is always shown (rule 4).
// While any action runs, every submit in the section is disabled (rule 5 needs that for an issue).
// Every submit control renders only after hydration (rule 7; plan SPEC DEVIATION 7 for the rest).
'use client';

import { useId, useRef, useState, useTransition, type ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import type { FormAction } from '@paigasus/console-core';
import type { PaigasusError } from '@paigasus/sdk/errors/types';
import { EmptyState, Field, Input, PRIMARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { linkHref } from '../../lib/paging';
import { PARENT_ARCHIVED_NOTE } from '../(console)/node-status';
import { serviceAccountIdOf } from '../(console)/service-accounts/service-account-id';
import type { OwnerKind, ReadOnlyView, SectionOk, SectionResult, ServiceAccountActions, ServiceAccountRowView, SimpleControl } from '../(console)/service-accounts/view';
import { FormError } from './form-error';
import { Pager } from './pager';
import { ServiceAccountPanel } from './service-account-panel';
import { TokenPanel, type TokenPanelHandle } from './token-panel';
import { useHydrated } from './use-hydrated';

export type ServiceAccountSectionProps = {
  readonly ownerKind: OwnerKind;
  /** The page's full path (/gateway/orgs/<org>[/projects/<project>]), for the select and pager links. */
  readonly path: string;
  readonly view: SectionOk;
  readonly actions: ServiceAccountActions;
};

// An issue success never reaches ResultMessage's success branch: runIssue below hands a successful
// token straight to the TokenPanel and never calls setResult for it (only an issue ERROR does). So
// this table excludes 'issue' — an entry for it would be dead code, never read (controller F9).
type SuccessControl = Exclude<SimpleControl, 'issue'>;

const SUCCESS_TEXT: Readonly<Record<SuccessControl, string>> = {
  allow: 'Model calls allowed.',
  revoke: 'Key revoked.',
  archive: 'Service account archived.',
};

/** § 5.3: a generic answer to "Allow model calls" can be a duplicate grant (§ 3.2). */
const ALLOW_MAY_ALREADY = 'Model calls may already be allowed. The page was reloaded.';
/** § 5.4: a plain denial and IAM's D15 check both answer forbidden. */
const ISSUE_FORBIDDEN = 'You need permission to issue keys here and to grant every role this account holds.';

function overrideFor(control: SimpleControl, error: PaigasusError): string | undefined {
  if (control === 'allow' && error.presentation === 'generic') return ALLOW_MAY_ALREADY;
  if (control === 'issue' && error.presentation === 'forbidden') return ISSUE_FORBIDDEN;
  return undefined;
}

/** § 5.8. `unknown` is read-only with no note. */
function readOnlyNote(readOnly: ReadOnlyView, ownerKind: OwnerKind): string | null {
  if (readOnly === 'archived') return `This ${ownerKind} is archived. IAM refuses changes to its service accounts and keys.`;
  if (readOnly === 'archived-parent') return PARENT_ARCHIVED_NOTE;
  return null;
}

function ResultMessage({ result, path, saOffset }: { readonly result: SectionResult; readonly path: string; readonly saOffset: number }): ReactElement | null {
  if (result === null) return null;
  if (result.control === 'create') {
    const state = result.state;
    if (state.kind === 'failed') return <FormError error={state.error} />;
    const id = serviceAccountIdOf(state.saPrn);
    const select =
      id === null ? null : (
        <ZoneLink prefetch={false} href={linkHref(path, { saOffset, sa: id })} className="underline">
          Select it
        </ZoneLink>
      );
    if (state.kind === 'partial') {
      return (
        <>
          <p role="status" className="flex flex-wrap gap-2 text-sm">
            <span>Service account created, but it cannot call models yet.</span>
            {select}
          </p>
          <FormError error={state.error} />
        </>
      );
    }
    return (
      <p role="status" className="flex flex-wrap gap-2 text-sm">
        <span>{state.granted ? 'Service account created. It can call models.' : 'Service account created. This IAM does not offer role administration, so it cannot call models from here.'}</span>
        {select}
      </p>
    );
  }
  if (result.state.ok) {
    // Structural, not merely a policy: an issue success never lands here (see SUCCESS_TEXT above),
    // and narrowing on it is what lets SUCCESS_TEXT's type omit an 'issue' entry.
    if (result.control === 'issue') return null;
    return (
      <p role="status" className="text-sm">
        {SUCCESS_TEXT[result.control]}
      </p>
    );
  }
  return <FormError error={result.state.error} message={overrideFor(result.control, result.state.error)} />;
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
  const [result, setResult] = useState<SectionResult>(null);
  const [working, startWork] = useTransition();
  const [issuing, startIssue] = useTransition();
  const generationRef = useRef(0);
  const tokenPanelRef = useRef<TokenPanelHandle>(null);
  const busy = working || issuing;
  const note = readOnlyNote(view.readOnly, ownerKind);

  function runCreate(form: FormData): void {
    generationRef.current += 1;
    const mine = generationRef.current;
    setResult(null);
    startWork(async () => {
      const state = await actions.create(null, form);
      if (state !== null && generationRef.current === mine) setResult({ control: 'create', state });
    });
  }

  function run(control: 'allow' | 'revoke' | 'archive', action: FormAction, form: FormData): void {
    generationRef.current += 1;
    const mine = generationRef.current;
    setResult(null);
    startWork(async () => {
      const state = await action(null, form);
      if (state !== null && generationRef.current === mine) setResult({ control, state });
    });
  }

  function runIssue(form: FormData): void {
    generationRef.current += 1;
    setResult(null);
    startIssue(async () => {
      const state = await actions.issue(null, form);
      // NO generation check (§ 5.4 rule 4): a token that IAM minted is always shown.
      if (state === null) return;
      if (state.ok) tokenPanelRef.current?.show(state.token, state.prefix);
      else setResult({ control: 'issue', state });
    });
  }

  return (
    <div className="flex flex-col gap-4">
      {note === null ? null : (
        <p data-testid="sa-read-only" className="text-muted-foreground text-sm">
          {note}
        </p>
      )}
      <div data-testid="sa-result" className="flex flex-col gap-2">
        <ResultMessage result={result} path={path} saOffset={view.saOffset} />
        <TokenPanel ref={tokenPanelRef} />
      </div>
      {view.canCreate && hydrated ? <CreateServiceAccountForm ownerPrn={view.ownerPrn} disabled={busy} onSubmit={runCreate} /> : null}
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
          run('allow', actions.allow, form);
        }}
        onRevoke={(form) => {
          run('revoke', actions.revoke, form);
        }}
        onArchive={(form) => {
          run('archive', actions.archive, form);
        }}
        onIssue={runIssue}
      />
    </div>
  );
}
