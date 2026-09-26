// SPDX-License-Identifier: Apache-2.0
//
// The body of "Model access for people" (SMA-676 spec § 4.4, § 5): the holders table with a Revoke
// per row, the candidate combobox with a Grant button, the empty states, the read-only note and the
// truncation lines. CLIENT component. Every submit control renders only after hydration (the rule of
// service-account-section.tsx). Each form shows its own error; a success revalidates the page, and
// the revalidated render moves the person between the two lists.
//
// STATE ACROSS A REVALIDATED RENDER. A `revalidatePath` follows every grant and revoke result, an
// error included (../(console)/people-model-access/actions.ts). React reconciles the refreshed
// server tree into the SAME client component instances where their key is unchanged, so a
// `useActionState` result survives that reconciliation. `HolderRow` is keyed by `row.grantId`: a
// failed revoke leaves the grant in place, so the key, and the error, stay. `GrantForm` is keyed by
// the candidate set: a failed grant leaves the candidates unchanged, so the key, and the error,
// stay; a SUCCESSFUL grant removes the chosen person from the candidates, changes the key, and the
// next form starts fresh with no error.
'use client';

import { useActionState, useState, type ReactElement } from 'react';
import { Combobox, EmptyState, PRIMARY_BUTTON_CLASS, SECONDARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { PEOPLE_COPY, type CandidateView, type HolderRowView, type PeopleModelAccessActions, type PeopleModelAccessOk } from '../(console)/people-model-access/view';
import { FormError } from './form-error';
import { useHydrated } from './use-hydrated';

function HolderRow({
  row,
  orgPrn,
  canRevoke,
  revoke,
}: {
  readonly row: HolderRowView;
  readonly orgPrn: string;
  readonly canRevoke: boolean;
  readonly revoke: PeopleModelAccessActions['revoke'];
}): ReactElement {
  const [state, formAction, pending] = useActionState(revoke, null);
  const hydrated = useHydrated();
  return (
    <TableRow data-testid="people-holder-row" data-principal={row.principalPrn}>
      <TableCell className="font-mono text-xs">{row.principalPrn}</TableCell>
      <TableCell>
        {row.member === false ? (
          <span data-testid="people-not-member" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
            {PEOPLE_COPY.notMember}
          </span>
        ) : null}
      </TableCell>
      <TableCell>
        {canRevoke && hydrated ? (
          <form action={formAction} aria-label={`Revoke model access of ${row.principalPrn}`}>
            <input type="hidden" name="principalPrn" value={row.principalPrn} />
            <input type="hidden" name="orgPrn" value={orgPrn} />
            <input type="hidden" name="grantId" value={row.grantId} />
            <button type="submit" disabled={pending} className={SECONDARY_BUTTON_CLASS}>
              {PEOPLE_COPY.revoke}
            </button>
            <FormError error={state?.ok === false ? state.error : null} />
          </form>
        ) : null}
      </TableCell>
    </TableRow>
  );
}

function GrantForm({ orgPrn, candidates, grant }: { readonly orgPrn: string; readonly candidates: readonly CandidateView[]; readonly grant: PeopleModelAccessActions['grant'] }): ReactElement {
  const [principalPrn, setPrincipalPrn] = useState('');
  const [state, formAction, pending] = useActionState(grant, null);
  return (
    <form action={formAction} aria-label={PEOPLE_COPY.grant} data-testid="people-grant-form" className="flex max-w-xl flex-col gap-3">
      <input type="hidden" name="orgPrn" value={orgPrn} />
      <input type="hidden" name="principalPrn" value={principalPrn} />
      <Combobox
        items={candidates.map((candidate) => ({ value: candidate.principalPrn, label: candidate.principalPrn }))}
        value={principalPrn}
        onValueChange={setPrincipalPrn}
        placeholder={PEOPLE_COPY.choose}
      />
      <button type="submit" disabled={pending || principalPrn === ''} className={PRIMARY_BUTTON_CLASS}>
        {PEOPLE_COPY.grant}
      </button>
      <FormError error={state?.ok === false ? state.error : null} />
    </form>
  );
}

export function PeopleModelAccessSection({ view, actions }: { readonly view: PeopleModelAccessOk; readonly actions: PeopleModelAccessActions }): ReactElement {
  const hydrated = useHydrated();
  return (
    <div className="flex flex-col gap-3">
      {view.readOnly ? (
        <p data-testid="people-read-only" className="text-muted-foreground text-sm">
          {PEOPLE_COPY.readOnly}
        </p>
      ) : null}
      {view.holders.length === 0 ? (
        <EmptyState title={PEOPLE_COPY.noHolders} />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Person</TableHead>
              <TableHead>Membership</TableHead>
              <TableHead>
                <span className="sr-only">{PEOPLE_COPY.revoke}</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {view.holders.map((row) => (
              <HolderRow key={row.grantId} row={row} orgPrn={view.orgPrn} canRevoke={view.flags.canRevoke} revoke={actions.revoke} />
            ))}
          </TableBody>
        </Table>
      )}
      {view.flags.grantsTruncated ? (
        <p data-testid="people-grants-truncated" className="text-muted-foreground text-sm">
          {PEOPLE_COPY.grantsTruncated}
        </p>
      ) : null}
      {view.flags.membersTruncated ? (
        <p data-testid="people-members-truncated" className="text-muted-foreground text-sm">
          {PEOPLE_COPY.membersTruncated}
        </p>
      ) : null}
      {view.candidates.length === 0 ? (
        <p className="text-muted-foreground text-sm">{PEOPLE_COPY.noCandidates}</p>
      ) : view.flags.canGrant && hydrated ? (
        // Keyed by the candidate set: after a grant the page revalidates, the chosen person leaves the
        // set, and a fresh form starts with no choice.
        <GrantForm key={view.candidates.map((candidate) => candidate.principalPrn).join(' ')} orgPrn={view.orgPrn} candidates={view.candidates} grant={actions.grant} />
      ) : null}
    </div>
  );
}
