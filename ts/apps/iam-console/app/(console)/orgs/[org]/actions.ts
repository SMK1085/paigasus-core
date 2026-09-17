// SPDX-License-Identifier: Apache-2.0
'use server';

// See ../actions.ts for the rules every action here follows. tests/unit/actions-structure.test.ts
// holds the iamClientsForAction() rule and bans the navigation helpers. The three lifecycle actions
// (SMA-630 spec § 4.4) also refresh the page after a forbidden or conflict answer, which often means
// that the page is stale.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '@paigasus/console-core';
import { formFields, invalidFormInput, refreshesAfterLifecycleAction } from '../../../../lib/form';
import { iamClientsForAction } from '../../../../lib/console';
import { TENANCY_PATH } from '../../../../lib/tenancy-path';
import { archiveOrganization, archiveOrganizationForm, createTeam, createTeamForm, renameOrganization, renameOrganizationForm, restoreOrganization, restoreOrganizationForm } from './commands';

export async function createTeamAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createTeamForm.safeParse(formFields(form, ['orgPrn', 'slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createTeam({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function renameOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = renameOrganizationForm.safeParse(formFields(form, ['prn', 'slug', 'name', 'currentSlug', 'currentName']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await renameOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function archiveOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = archiveOrganizationForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function restoreOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = restoreOrganizationForm.safeParse(formFields(form, ['prn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await restoreOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (refreshesAfterLifecycleAction(result)) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
