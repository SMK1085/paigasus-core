// SPDX-License-Identifier: Apache-2.0
'use server';

// The two Server Actions of "Model access for people" (SMA-676 spec § 4.4). Each follows the rule
// of ../service-accounts/actions.ts, in this order: iamClientsForAction() first; then a zod check of
// ONLY the fields it accepts (formFields names them); then a pure command. No action consults mayI():
// IAM decides. tests/unit/actions-structure.test.ts holds these rules.
//
// Revalidation follows ../service-accounts/revalidation.ts: after every result except `relogin`.
import { revalidatePath } from 'next/cache';
import { formFields, invalidFormInput, type ActionState } from '@paigasus/console-core';
import { iamClientsForAction } from '../../../lib/console';
import { SETTINGS_PATH } from '../../../lib/settings-path';
import { refreshesAfterMutation } from '../service-accounts/revalidation';
import { grantModelAccess, grantModelAccessForm, revokeModelAccess, revokeModelAccessForm } from './commands';

export async function grantModelAccessAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = grantModelAccessForm.safeParse(formFields(form, ['principalPrn', 'orgPrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await grantModelAccess({ authz: clients.value.authz }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function revokeModelAccessAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = revokeModelAccessForm.safeParse(formFields(form, ['principalPrn', 'orgPrn', 'grantId']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await revokeModelAccess({ authz: clients.value.authz }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}
