// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClients() rule.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../../lib/errors';
import { formFields, invalidFormInput } from '../../../../lib/form';
import { iamClients } from '../../../../lib/iam';
import { createTeam, createTeamForm } from './commands';

export async function createTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = createTeamForm.safeParse(formFields(form, ['orgPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createTeam({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath('/orgs', 'layout');
  return result;
}
