// SPDX-License-Identifier: Apache-2.0
'use server';

// Server Action shells (spec § 5.3). Each one gets its client through iamClients(), so
// requireSession() runs for every action. tests/unit/actions-structure.test.ts checks that every
// export calls it. No action consults mayI(): a hidden button is cosmetic, and IAM decides
// (spec § 6.3). The same test checks that no code in this file names it.
import { revalidatePath } from 'next/cache';
import type { ActionState } from '../../../lib/errors';
import { formFields, invalidFormInput } from '../../../lib/form';
import { iamClients } from '../../../lib/iam';
import { createOrganization, createOrganizationForm } from './commands';

/**
 * basePath-relative. Route groups such as (console) are not part of the URL path, so '/orgs' with
 * 'layout' covers every page under /orgs. Task 22's e2e test asserts that a created organization appears.
 */
const TENANCY_PATH = '/orgs';

export async function createOrganizationAction(_previous: ActionState, form: FormData): Promise<ActionState> {
  const clients = await iamClients();
  const parsed = createOrganizationForm.safeParse(formFields(form, ['slug', 'name']));
  if (!parsed.success) return { ok: false, error: invalidFormInput() };
  const result = await createOrganization({ tenancy: clients.tenancy }, parsed.data);
  if (result.ok) revalidatePath(TENANCY_PATH, 'layout');
  return result;
}
