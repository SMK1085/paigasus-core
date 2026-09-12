// SPDX-License-Identifier: Apache-2.0
//
// The commands behind this folder's Server Actions (spec § 5.3). A command takes its IAM client as
// a port, so the tier-2 tests call it against the fake IAM with no session and no Next runtime.
// It takes NO mayI: a command cannot refuse an action on the UI's guess. IAM decides (spec § 6.3).
import 'server-only';
import { z } from 'zod';
import { callIam } from '../../../lib/errors';
import { toActionResult, type ActionResult } from '../../../lib/form';
import type { IamClients } from '../../../lib/iam';

const text = z.string().trim().min(1).max(200);

/** The shape of the form, not IAM's rules. IAM validates the slug grammar and answers with a reason. */
export const createOrganizationForm = z.object({ slug: text, name: text });
export type CreateOrganizationInput = z.infer<typeof createOrganizationForm>;

export async function createOrganization(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'createOrganization'> }, input: CreateOrganizationInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.createOrganization({ slug: input.slug, name: input.name })));
}
