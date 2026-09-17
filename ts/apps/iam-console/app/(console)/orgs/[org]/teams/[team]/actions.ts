// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../../../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClientsForAction() rule and bans the navigation helpers. The three lifecycle actions
// (SMA-630 spec § 4.4) also refresh the page after a forbidden or conflict answer.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '@paigasus/console-core';
import { formFields, invalidFormInput, refreshesAfterLifecycleAction } from '../../../../../../lib/form';
import { iamClientsForAction } from '../../../../../../lib/console';
import { TENANCY_PATH } from '../../../../../../lib/tenancy-path';
import { archiveTeam, archiveTeamForm, createProject, createProjectForm, renameTeam, renameTeamForm, restoreTeam, restoreTeamForm } from './commands';

export async function createProjectAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createProjectForm.safeParse(formFields(form, ['teamPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createProject({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function renameTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = renameTeamForm.safeParse(formFields(form, ['prn', 'slug', 'name', 'currentSlug', 'currentName']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await renameTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function archiveTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = archiveTeamForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function restoreTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = restoreTeamForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await restoreTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
