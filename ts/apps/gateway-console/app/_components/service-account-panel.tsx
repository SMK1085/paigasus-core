// SPDX-License-Identifier: Apache-2.0
//
// The selected service account (SMA-636 spec § 4.6, D14): "Can call models", "Allow model calls",
// the issue-key form, the keys list with its pager, and "Archive". CLIENT component. It holds NO
// result: every control hands its form to the section, whose ONE result region shows every result.
// The loader decided which controls show (PanelControls); this component only renders them.
'use client';

import { useId, type FormEvent, type ReactElement } from 'react';
import { EmptyState, PRIMARY_BUTTON_CLASS, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@paigasus/ui';
import { EXPIRY_CHOICES, EXPIRY_LABEL, KEY_STATUS_LABEL, type KeysView, type ModelCallState, type SelectedView } from '../(console)/service-accounts/view';
import { ConfirmButton } from './confirm-button';
import { FormError } from './form-error';
import { Pager } from './pager';

export type ServiceAccountPanelProps = {
  readonly selected: SelectedView;
  readonly path: string;
  readonly saOffset: number;
  readonly sa: string | null;
  /** Submit controls render only after hydration (§ 5.4 rule 7, plan SPEC DEVIATION 7). */
  readonly hydrated: boolean;
  /** true while any action of the section runs (§ 5.4 rule 5). */
  readonly disabled: boolean;
  readonly onAllow: (form: FormData) => void;
  readonly onIssue: (form: FormData) => void;
  readonly onRevoke: (form: FormData) => void;
  readonly onArchive: (form: FormData) => void;
};

const MODEL_CALLS_TEXT: Readonly<Record<ModelCallState, string>> = { yes: 'Yes', no: 'No', archived: 'No (account archived)', unknown: 'Unknown' };

/** § 5.6: the exact confirm text. */
export const ARCHIVE_CONFIRMATION = 'All keys of this account stop working. You cannot undo this in the console.';

function submitting(onSubmit: (form: FormData) => void) {
  return (event: FormEvent<HTMLFormElement>): void => {
    event.preventDefault();
    onSubmit(new FormData(event.currentTarget));
  };
}

function AllowModelCallsForm({ saPrn, disabled, onSubmit }: { readonly saPrn: string; readonly disabled: boolean; readonly onSubmit: (form: FormData) => void }): ReactElement {
  return (
    <form aria-label="Allow model calls" onSubmit={submitting(onSubmit)}>
      <input type="hidden" name="saPrn" value={saPrn} />
      <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
        Allow model calls
      </button>
    </form>
  );
}

/** § 5.4: the expiry choice only (D7). The server computes the date, and the scope comes from IAM (D6). */
export function IssueKeyForm({ saPrn, disabled, onSubmit }: { readonly saPrn: string; readonly disabled: boolean; readonly onSubmit: (form: FormData) => void }): ReactElement {
  const id = useId();
  return (
    <form aria-label="Issue API key" data-testid="issue-key-form" className="flex max-w-md flex-col gap-2" onSubmit={submitting(onSubmit)}>
      <input type="hidden" name="saPrn" value={saPrn} />
      <label htmlFor={`${id}-expiry`} className="text-sm font-medium">
        Expiry
      </label>
      <select id={`${id}-expiry`} name="expiry" defaultValue="90" className="border-input rounded-pgs border px-2 py-1 text-sm">
        {EXPIRY_CHOICES.map((choice) => (
          <option key={choice} value={choice}>
            {EXPIRY_LABEL[choice]}
          </option>
        ))}
      </select>
      <button type="submit" disabled={disabled} className={PRIMARY_BUTTON_CLASS}>
        Issue key
      </button>
    </form>
  );
}

export function ArchiveServiceAccountButton({ saPrn, disabled, onConfirm }: { readonly saPrn: string; readonly disabled: boolean; readonly onConfirm: (form: FormData) => void }): ReactElement {
  return (
    <ConfirmButton testId="archive-service-account" label="Archive" confirmLabel="Confirm archive" confirmation={ARCHIVE_CONFIRMATION} hidden={{ saPrn }} disabled={disabled} onConfirm={onConfirm} />
  );
}

export function RevokeKeyButton({
  saPrn,
  keyId,
  prefix,
  disabled,
  onConfirm,
}: {
  readonly saPrn: string;
  readonly keyId: string;
  readonly prefix: string;
  readonly disabled: boolean;
  readonly onConfirm: (form: FormData) => void;
}): ReactElement {
  return (
    <ConfirmButton
      testId={`revoke-key-${keyId}`}
      label="Revoke"
      confirmLabel="Confirm revoke"
      confirmation={`Revoke the key ${prefix}? A client that sends it can no longer call models.`}
      hidden={{ saPrn, keyId }}
      disabled={disabled}
      onConfirm={onConfirm}
    />
  );
}

type KeysListProps = {
  readonly keys: KeysView;
  readonly saPrn: string;
  readonly path: string;
  readonly saOffset: number;
  readonly sa: string | null;
  readonly revoke: boolean;
  readonly disabled: boolean;
  readonly onRevoke: (form: FormData) => void;
};

/** § 5.7: IAM's order, with the pager. Columns: prefix, status, created, expires, last used. */
function KeysList({ keys, saPrn, path, saOffset, sa, revoke, disabled, onRevoke }: KeysListProps): ReactElement | null {
  if (keys.kind === 'hidden') return null;
  if (keys.kind === 'error') {
    return (
      <div data-testid="api-keys-error">
        <FormError error={keys.error} />
      </div>
    );
  }
  return (
    <div data-testid="api-keys" className="flex flex-col gap-2">
      <h4 className="text-sm font-medium">API keys</h4>
      {keys.rows.length === 0 && keys.page.offset === 0 ? (
        <EmptyState title="No API keys yet" />
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Prefix</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Created</TableHead>
              <TableHead>Expires</TableHead>
              <TableHead>Last used</TableHead>
              <TableHead>
                <span className="sr-only">Revoke</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {keys.rows.map((key) => (
              <TableRow key={key.id} data-testid="api-key-row" data-status={key.status}>
                <TableCell>
                  <code>{key.prefix}</code>
                  {key.otherScope === null ? null : (
                    <p data-testid="api-key-scope" className="text-muted-foreground text-xs">
                      {`Scope: ${key.otherScope}. The gateway checks model calls against this scope.`}
                    </p>
                  )}
                </TableCell>
                <TableCell>{KEY_STATUS_LABEL[key.status]}</TableCell>
                <TableCell>{key.created ?? ''}</TableCell>
                <TableCell>{key.expires ?? 'Never'}</TableCell>
                <TableCell>{key.lastUsed ?? 'Never'}</TableCell>
                <TableCell>{revoke && key.status === 'active' ? <RevokeKeyButton saPrn={saPrn} keyId={key.id} prefix={key.prefix} disabled={disabled} onConfirm={onRevoke} /> : null}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <Pager label="API key pages" path={path} param="keyOffset" offset={keys.page.offset} nextOffset={keys.page.nextOffset} keep={{ saOffset, sa }} />
    </div>
  );
}

export function ServiceAccountPanel(props: ServiceAccountPanelProps): ReactElement | null {
  const { selected } = props;
  if (selected.kind === 'none') return null;
  if (selected.kind === 'other-scope') {
    return (
      <p data-testid="sa-panel-other" className="text-muted-foreground text-sm">
        This service account belongs to another scope.
      </p>
    );
  }
  if (selected.kind === 'error') {
    return (
      <div data-testid="sa-panel-error">
        <FormError error={selected.error} />
      </div>
    );
  }
  const { account, modelCalls, keys, controls } = selected;
  return (
    <section aria-label={`Service account ${account.name}`} data-testid="sa-panel" className="flex flex-col gap-3 border-t pt-4">
      <div className="flex flex-wrap items-center gap-2">
        <h3 className="text-base font-semibold">{account.name}</h3>
        {account.active ? null : (
          <span data-testid="sa-panel-archived" className="border-input text-muted-foreground rounded-pgs border px-2 py-0.5 text-xs font-medium">
            Archived
          </span>
        )}
      </div>
      <p data-testid="model-calls" data-state={modelCalls} className="text-sm">
        {`Can call models: ${MODEL_CALLS_TEXT[modelCalls]}`}
      </p>
      {controls.allow && props.hydrated ? <AllowModelCallsForm saPrn={account.prn} disabled={props.disabled} onSubmit={props.onAllow} /> : null}
      {controls.issue && props.hydrated ? <IssueKeyForm saPrn={account.prn} disabled={props.disabled} onSubmit={props.onIssue} /> : null}
      <KeysList keys={keys} saPrn={account.prn} path={props.path} saOffset={props.saOffset} sa={props.sa} revoke={controls.revoke} disabled={props.disabled} onRevoke={props.onRevoke} />
      {controls.archive ? <ArchiveServiceAccountButton saPrn={account.prn} disabled={props.disabled} onConfirm={props.onArchive} /> : null}
    </section>
  );
}
