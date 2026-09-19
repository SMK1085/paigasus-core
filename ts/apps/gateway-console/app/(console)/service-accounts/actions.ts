// SPDX-License-Identifier: Apache-2.0
'use server';

// The five Server Actions of the gateway settings (SMA-636 spec § 5). Both settings pages pass
// these same five to ServiceAccountSection (plan SPEC DEVIATION 5): an action takes its owner from
// the form (create) or from IAM (D6), never from the page.
//
// Every action follows § 5.1, in this order: iamClientsForAction() first; then a zod shape check
// of ONLY the fields it accepts (formFields names them, and the rest of the form is ignored); then
// a pure command. No action consults mayI(): IAM decides. tests/unit/actions-structure.test.ts
// holds these rules.
//
// Revalidation follows ./revalidation.ts. A `relogin` result returns before any revalidatePath.
//
// THE TOKEN (§ 5.4 rule 1). issueApiKeyAction returns the plaintext token in its result, and that
// result is the ONE response body that may carry it. Nothing here logs it or puts it in a URL, a
// cookie or page data.
import { revalidatePath } from 'next/cache';
import { cedarCapabilityOf, formFields, invalidFormInput, logger, type ActionState } from '@paigasus/console-core';
import { discovery, iamClientsForAction, optionalSession } from '../../../lib/console';
import { SETTINGS_PATH } from '../../../lib/settings-path';
import { allowModelCalls, archiveServiceAccount, createServiceAccount, createServiceAccountForm, issueApiKey, issueApiKeyForm, revokeApiKey, revokeApiKeyForm, serviceAccountForm } from './commands';
import { refreshesAfterCreate, refreshesAfterIssue, refreshesAfterMutation } from './revalidation';
import type { CreateState, IssueKeyState } from './view';

/** D13: grant only when IAM is available and reports iam.authz.cedar (the rule of cedarCapabilityOf). */
async function roleAdministrationOffered(): Promise<boolean> {
  const session = await optionalSession();
  if (session === null) return false;
  return cedarCapabilityOf(await discovery().getServiceState('iam', session.accessToken));
}

export async function createServiceAccountAction(_previous: CreateState, form: FormData): Promise<CreateState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return { kind: 'failed', error: clients.error };
  const parsed = createServiceAccountForm.safeParse(formFields(form, ['ownerPrn', 'name']));
  if (!parsed.success) return { kind: 'failed', error: invalidFormInput() };
  const result = await createServiceAccount({ serviceAccounts: clients.value.serviceAccounts, authz: clients.value.authz, cedar: await roleAdministrationOffered(), logger }, parsed.data);
  if (refreshesAfterCreate(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function allowModelCallsAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = serviceAccountForm.safeParse(formFields(form, ['saPrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await allowModelCalls({ serviceAccounts: clients.value.serviceAccounts, authz: clients.value.authz }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

/** § 5.4 rule 2: the client calls this DIRECTLY in a transition, with `null` as the previous state. */
export async function issueApiKeyAction(_previous: null, form: FormData): Promise<IssueKeyState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = issueApiKeyForm.safeParse(formFields(form, ['saPrn', 'expiry']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await issueApiKey({ serviceAccounts: clients.value.serviceAccounts, now: Date.now }, parsed.data);
  if (refreshesAfterIssue(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function revokeApiKeyAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = revokeApiKeyForm.safeParse(formFields(form, ['saPrn', 'keyId']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await revokeApiKey({ serviceAccounts: clients.value.serviceAccounts }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}

export async function archiveServiceAccountAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = serviceAccountForm.safeParse(formFields(form, ['saPrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await archiveServiceAccount({ serviceAccounts: clients.value.serviceAccounts }, parsed.data);
  if (refreshesAfterMutation(result)) revalidatePath(SETTINGS_PATH, 'layout');
  return result;
}
