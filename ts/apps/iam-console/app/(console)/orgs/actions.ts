// SPDX-License-Identifier: Apache-2.0
'use server';

// Server Action shells (spec § 5.3). Each one gets its client through iamClientsForAction(), which
// reads the session WITHOUT a redirect: a Server Action's redirect leaves the zone (see that
// function's doc comment). tests/unit/actions-structure.test.ts checks that every export calls it,
// and that no export calls the redirecting iamClients(). No action consults mayI(): a hidden button
// is cosmetic, and IAM decides (spec § 6.3). The same test checks that no code in this file names it.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../lib/errors';
import { formFields, invalidFormInput } from '../../../lib/form';
import { iamClientsForAction } from '../../../lib/iam';
import { attachMembership, attachMembershipForm, createOrganization, createOrganizationForm, detachMembership, detachMembershipForm } from './commands';

/**
 * basePath-relative. Route groups such as (console) are not part of the URL path, so '/orgs' with
 * 'layout' covers every page under /orgs. Task 22's e2e test asserts that a created organization appears.
 */
const TENANCY_PATH = '/orgs';

export async function createOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = createOrganizationForm.safeParse(formFields(form, ['slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createOrganization({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function attachMembershipAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = attachMembershipForm.safeParse(formFields(form, ['principalPrn', 'nodePrn']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await attachMembership({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}

export async function detachMembershipAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClientsForAction();
  if (!clients.ok) return clients;
  const parsed = detachMembershipForm.safeParse(formFields(form, ['id']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await detachMembership({ tenancy: clients.value.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
