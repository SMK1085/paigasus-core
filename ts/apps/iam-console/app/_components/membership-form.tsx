// SPDX-License-Identifier: Apache-2.0
'use client';

import { useActionState, useId, type ReactElement } from 'react';
import { Field, Input } from '@paigasus/ui';
import type { ActionState } from '../../lib/errors';
import { FormError } from './form-error';

export type MembershipAction = (previous: ActionState, form: FormData) => Promise<ActionState>;

/**
 * Attach by raw principal PRN: IAM has no user lookup RPC (spec § 5.2, a recorded limit, § 12).
 * The page renders this only when mayI('AttachMembership', node) said yes. The action still asks IAM.
 */
export function AttachMembershipForm({ nodePrn, action }: { readonly nodePrn: string; readonly action: MembershipAction }): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  const id = useId();
  return (
    <form action={formAction} aria-label="Add member" data-testid="attach-membership" className="flex max-w-md flex-col gap-3">
      <input type="hidden" name="nodePrn" value={nodePrn} />
      <Field label="Principal PRN" htmlFor={`${id}-principal`} description="IAM has no user search yet. Enter the PRN of the principal.">
        <Input name="principalPrn" required autoComplete="off" />
      </Field>
      <button type="submit" disabled={pending} className="bg-primary text-primary-foreground rounded-pgs self-start px-3 py-1.5 text-sm font-medium disabled:opacity-50">
        Add member
      </button>
      {state?.ok === true ? (
        <p role="status" className="text-sm">
          Added.
        </p>
      ) : null}
      <div data-testid="attach-membership-error">
        <FormError error={state?.ok === false ? state.error : null} />
      </div>
    </form>
  );
}

export function DetachMembershipButton({ membershipId, principalPrn, action }: { readonly membershipId: string; readonly principalPrn: string; readonly action: MembershipAction }): ReactElement {
  const [state, formAction, pending] = useActionState(action, null);
  return (
    <form action={formAction} aria-label={`Remove ${principalPrn}`} className="flex flex-col gap-1">
      <input type="hidden" name="id" value={membershipId} />
      <button type="submit" disabled={pending} className="text-destructive self-start text-sm underline disabled:opacity-50">
        Remove
      </button>
      <FormError error={state?.ok === false ? state.error : null} />
    </form>
  );
}
