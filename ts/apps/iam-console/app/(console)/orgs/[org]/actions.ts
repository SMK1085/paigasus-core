// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClientsForAction() rule.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../../lib/errors';
import { formFields, invalidFormInput } from '../../../../lib/form';
import { iamClientsForAction } from '../../../../lib/iam';
import { TENANCY_PATH } from '../../../../lib/tenancy-path';
import { createTeam, createTeamForm } from './commands';

export async function createTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createTeamForm.safeParse(formFields(form, ['orgPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
