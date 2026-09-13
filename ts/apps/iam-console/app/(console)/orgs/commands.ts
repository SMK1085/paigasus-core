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

/** A PRN field: bounded text. IAM parses it and answers `invalid-prn` for a bad one. */
const prn = z.string().trim().min(1).max(512);

export const attachMembershipForm = z.object({ principalPrn: prn, nodePrn: prn });
export type AttachMembershipInput = z.infer<typeof attachMembershipForm>;

/** The same for every node: the node PRN is a hidden field of the page that rendered the form. */
export async function attachMembership(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'attachMembership'> }, input: AttachMembershipInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.attachMembership({ principalPrn: input.principalPrn, nodePrn: input.nodePrn })));
}

export const detachMembershipForm = z.object({ id: z.string().trim().min(1).max(64) });
export type DetachMembershipInput = z.infer<typeof detachMembershipForm>;

/** IAM finds the membership's node itself and checks DetachMembership against it. */
export async function detachMembership(deps: { readonly tenancy: Pick<IamClients['tenancy'], 'detachMembership'> }, input: DetachMembershipInput): Promise<ActionResult> {
  return toActionResult(await callIam(() => deps.tenancy.detachMembership({ id: input.id })));
}
