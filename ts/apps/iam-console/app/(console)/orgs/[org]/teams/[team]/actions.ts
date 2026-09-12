// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../../../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClientsForAction() rule.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../../../../lib/errors';
import { formFields, invalidFormInput } from '../../../../../../lib/form';
import { iamClientsForAction } from '../../../../../../lib/iam';
import { createProject, createProjectForm } from './commands';

export async function createProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createProjectForm.safeParse(formFields(form, ['teamPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createProject({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath('/orgs', 'layout');
  return result;
}
